use super::super::test_host::{FileHost, canonical_tempdir};
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_core::host::Host as _;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[track_caller]
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

fn start_active_markdown_in_source(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, _window, cx| {
        let view = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone();
        view.update(cx, |view, cx| {
            if view.is_live() {
                view.toggle_mode(cx);
            }
            assert!(!view.is_live());
        });
    });
}

#[gpui_kit::test]
fn terminal_single_line_citation_click_opens_the_editor(cx: &mut TestAppContext) {
    terminal_citation_click_opens_the_editor(cx, "(citation.txt:3),\r\n", false);
}

#[gpui_kit::test]
fn terminal_range_citation_click_opens_the_editor_at_its_first_line(cx: &mut TestAppContext) {
    terminal_citation_click_opens_the_editor(cx, "(citation.txt:3-5),\r\n", false);
}

#[gpui_kit::test]
fn terminal_citation_spellings_keep_first_line_navigation(cx: &mut TestAppContext) {
    for output in [
        "(citation.txt:3:1),\r\n",
        "(citation.txt(3,1)),\r\n",
        "(citation.txt#L3-L5),\r\n",
        "(\"citation.txt:3-5\"),\r\n",
        "({absolute}:3-5),\r\n",
    ] {
        terminal_citation_click_opens_the_editor(cx, output, false);
    }
}

#[gpui_kit::test]
fn terminal_range_citation_opens_on_the_panes_host(cx: &mut TestAppContext) {
    terminal_citation_click_opens_the_editor(cx, "(citation.txt:3-5),\r\n", true);
}

fn terminal_citation_click_opens_the_editor(cx: &mut TestAppContext, output: &str, remote: bool) {
    use crate::daemon::protocol::DaemonMsg;
    use gpui_kit::{Modifiers, MouseButton};
    use rio_vt::crosswords::pos::{Column, Line};

    let dir = canonical_tempdir();
    let path = dir.path().join("citation.txt");
    std::fs::write(&path, "first\nsecond\nrange starts here\nfourth\nfifth\n").unwrap();
    let output = output.replace("{absolute}", &path.to_string_lossy());
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    let pane = app.read_with(&vcx, |app, _| app.tabs[0].pane.terminals()[0].clone());
    let (cwd, expected_path, expected_host) = if remote {
        use crate::core::session::{
            RemoteRef, RemoteTarget, WindowView, WorkspaceId, WorkspaceStore,
        };
        use crate::terminal::PaneWorkspace;
        use crate::ui::host_registry::HostRegistry;

        let target = RemoteTarget::Alias {
            alias: "range-citation-fixture".into(),
        };
        let host_id = target.host_id();
        app.update_in(&mut vcx, |app, _, cx| {
            let workspace = WorkspaceId::new();
            let view = WindowView::on_remote(RemoteRef::new(target.clone(), workspace));
            app.workspace = view.id;
            let mut views = WorkspaceStore::all(cx).clone();
            views.views.push(view);
            WorkspaceStore::install_for_test(cx, views);
            HostRegistry::insert(cx, FileHost::mapped(host_id, dir.path()));
            pane.update(cx, |pane, _| {
                pane.set_workspace(Some(PaneWorkspace {
                    workspace,
                    target,
                    spec: None,
                    label: None,
                    resize_echo: false,
                }));
            });
        });
        (PathBuf::from("/"), PathBuf::from("/citation.txt"), host_id)
    } else {
        (dir.path().to_path_buf(), path.clone(), HostId::LOCAL)
    };
    DaemonMsg::Cwd(cwd.clone()).encode(&mut streams[0]).unwrap();
    DaemonMsg::Output(output.as_bytes().to_vec())
        .encode(&mut streams[0])
        .unwrap();
    wait_until(&mut vcx, |vcx| {
        pane.read_with(vcx, |pane, _| {
            pane.cwd().as_deref() == Some(cwd.as_path())
                && pane.terminal.term.lock().grid[Line(0)][Column(0)].c() == '('
        })
    });
    let at = vcx.update(|window, cx| {
        window.render_frame(cx);
        // The first row's path starts after `(`, inside the surface padding.
        let bounds = window.find("terminal-surface").bounds();
        let width = pane.read(cx).cell_width;
        bounds.origin + gpui_kit::point(gpui_kit::px(8.) + width * 2., gpui_kit::px(6.))
    });
    let modifier = if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::none()
        }
    } else {
        Modifiers::control()
    };
    vcx.simulate_mouse_move(at, None, modifier);
    if remote {
        wait_until(&mut vcx, |vcx| {
            pane.update_in(vcx, |pane, _, cx| pane.hover_link_at(2, 0, true, cx))
        });
    }
    vcx.simulate_mouse_down(at, MouseButton::Left, modifier);
    vcx.simulate_mouse_up(at, MouseButton::Left, modifier);
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        let file = app
            .active_buffer()
            .expect("citation opens the production editor");
        assert_eq!(file.path, expected_path);
        assert_eq!(file.host.id(), expected_host);
        assert_eq!(file.input.text().unwrap().read(cx).cursor_position(), Position::new(2, 0));
        assert_eq!(
            file.input.text().unwrap().read(cx).text().to_string(),
            "first\nsecond\nrange starts here\nfourth\nfifth\n"
        );
    });
}

#[gpui_kit::test]
fn production_markdown_opens_live_without_changing_code_or_plain_untitled(cx: &mut TestAppContext) {
    use crate::core::actions::{EditorNewFile, EditorNewMarkdownFile};
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    for extension in ["md", "MARKDOWN", "MDX"] {
        let path = dir.path().join(format!("named.{extension}"));
        std::fs::write(&path, "# Heading\n\nbody\n").unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        wait_until(&mut vcx, |vcx| {
            app.read_with(vcx, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| file.path.file_name() == path.file_name())
            })
        });
        app.read_with(&vcx, |app, cx| {
            let file = app.active_buffer().unwrap();
            assert_eq!(file.language(), "markdown");
            assert_eq!(
                file.input.markdown().unwrap().read(cx).is_live(),
                !extension.eq_ignore_ascii_case("mdx")
            );
            assert_eq!(
                file.input.text().unwrap().read(cx).text().to_string(),
                "# Heading\n\nbody\n"
            );
            assert!(!file.dirty);
        });
    }
    let code = dir.path().join("main.rs");
    std::fs::write(&code, "fn main() {}\n").unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&code, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.input.code().is_some())
        })
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.toggle_document_preview(window, cx)
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.language(), "rust");
        assert!(file.input.code().is_some());
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "fn main() {}\n");
    });
    vcx.dispatch_action(EditorNewFile);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        let file = app.active_buffer().unwrap();
        assert!(file.untitled.is_some());
        assert!(file.path.as_os_str().is_empty());
        assert!(file.input.code().is_some());
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_scrollback("# terminal output".into(), window, cx)
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(file.input.code().is_some());
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "# terminal output");
    });
    vcx.dispatch_action(EditorNewMarkdownFile);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(file.untitled.is_some());
        assert!(file.path.as_os_str().is_empty());
        let view = file.input.markdown().unwrap().read(cx);
        assert!(view.is_live());
        assert_eq!(view.document().read(cx).source().to_string(), "");
        assert!(!file.dirty);
    });
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
                .text()
                .unwrap()
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
            .text()
            .unwrap()
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
                .text()
                .unwrap()
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
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), edited);
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
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
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
    start_active_markdown_in_source(&app, &mut vcx);
    save(&mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.active_buffer().unwrap().host.id(), HostId(705));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .text()
                .unwrap()
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
    for (index, &(original, inserted, bom_len)) in fixtures.iter().enumerate() {
        let path = dir.path().join(format!("encoding-{index}.md"));
        std::fs::write(&path, original).unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_open_on_host(host.clone(), &path, window, cx)
        });
        wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
        start_active_markdown_in_source(&app, &mut vcx);
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
        start_active_markdown_in_source(&app, &mut vcx);
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
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
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
        assert_eq!(file.format(cx).unwrap().encoding_label(), "Windows-1252");
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "🎉café  \nlast");
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
    let invalid: &[&[u8]] = &[
        b"\xef\xbb\xbfbroken\xff",
        b"\xff\xfea\0\x00\xd8",
        b"\xfe\xff\0a\xd8\x00",
        b"\xff\xfea",
    ];
    for (index, bytes) in invalid.iter().enumerate() {
        let name = format!("damaged-{index}.{}", ["md", "markdown", "MDX", "md"][index]);
        let path = dir.path().join(&name);
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
                        record.message().contains(&name)
                            && record.message().contains("invalid source bytes")
                    })
            })
        });
        assert_eq!(std::fs::read(&path).unwrap(), *bytes);
        assert_eq!(active_id(&app, &vcx), None);
    }
}

#[gpui_kit::test]
fn markdown_change_commands_target_focused_split_presentation(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("split-changes.md");
    std::fs::write(&path, "one\nTWO\nthree\nfour\nfive\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
    let first = app.read_with(&vcx, |app, _| app.active_editor_input().unwrap());
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    start_active_markdown_in_source(&app, &mut vcx);
    vcx.update(|window, cx| window.render_frame(cx));
    let second = app.read_with(&vcx, |app, _| app.active_editor_input().unwrap());
    app.update_in(&mut vcx, |app, window, cx| {
        let id = app.active_buffer().unwrap().id();
        app.editor
            .buffers
            .get_mut(id)
            .unwrap()
            .gutter
            .set_base_for_test("one\ntwo\nthree\nfour\n");
        first.text().unwrap().set_cursor_position(Position::new(0, 0), window, cx);
        second.text().unwrap().set_cursor_position(Position::new(1, 0), window, cx);
        second.focus(window, cx);
        assert!(app.editor_gutter_step(true, window, cx));
        assert_eq!(second.text().unwrap().read(cx).cursor_position().line, 4);
        assert_eq!(first.text().unwrap().read(cx).cursor_position().line, 0);
        assert!(app.editor_gutter_revert_at_cursor(window, cx));
        assert_eq!(
            second.text().unwrap().read(cx).text().to_string(),
            "one\nTWO\nthree\nfour\n"
        );
        assert_eq!(first.text().unwrap().read(cx).cursor_position().line, 0);
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    app.read_with(&vcx, |_, cx| {
        assert_eq!(
            second.text().unwrap().read(cx).text().to_string(),
            "one\nTWO\nthree\nfour\nfive\n"
        )
    });
}

#[gpui_kit::test]
fn markdown_workbench_shares_native_source_history_and_host_save_across_groups(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.mdx");
    std::fs::write(&path, "alpha\nbeta\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
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
    start_active_markdown_in_source(&app, &mut vcx);
    vcx.update(|window, cx| window.render_frame(cx));
    let second = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.dispatch_action(gpui_kit::component::input::MoveToStart);
    vcx.update(|_, cx| {
        assert_ne!(first.entity_id(), second.entity_id());
        assert_eq!(
            first.read(cx).document().entity_id(),
            second.read(cx).document().entity_id()
        );
        assert_eq!(first.read(cx).cursor(), 7);
        assert_eq!(second.read(cx).cursor(), 0);
    });
    vcx.dispatch_action(gpui_kit::component::input::SelectAll);
    vcx.dispatch_action(gpui_kit::component::input::Copy);
    wait_until(&mut vcx, |context| {
        context.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref()
                == Some("中🎉alpha\nbeta\n")
        })
    });
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("中🎉alpha\nbeta\n".into())
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Cut);
    wait_until(&mut vcx, |context| {
        first.read_with(context, |view, cx| {
            view.document().read(cx).source().len() == 0
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .text()
                .unwrap()
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
                .text()
                .unwrap()
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
                .text()
                .unwrap()
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
                .text()
                .unwrap()
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
    start_active_markdown_in_source(&app, &mut vcx);
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_ne!(file.id(), id);
        assert!(file.input.code().is_none());
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "中🎉alpha\nbeta\n");
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
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
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
    });
    // Source projection and native coordinates are revision-scoped; a commit
    // may schedule another frame before its bounds can be queried.
    wait_until(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            view.update(cx, |view, cx| {
                let Some(start) =
                    view.bounds_for_range(0..0, gpui_kit::Bounds::default(), window, cx)
                else {
                    return false;
                };
                let Some(committed) =
                    view.bounds_for_range(3..3, gpui_kit::Bounds::default(), window, cx)
                else {
                    return false;
                };
                assert!(committed.left() > start.left());
                assert_eq!(committed.top(), start.top());
                assert!(committed.size.height > gpui_kit::px(0.));
                true
            })
        })
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .text()
                .unwrap()
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
                .is_some_and(|file| file.input.text().unwrap().read(cx).text().to_string() == "external version\n")
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
            .text()
            .unwrap()
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
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), text);
        assert!(app.editor.is_dirty(HostId::LOCAL, &file.path));
    });
}

#[gpui_kit::test]
fn markdown_reload_started_before_composition_reports_conflict_without_losing_input(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{EntityInputHandler as _, Focusable as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending-composition.md");
    let original = "alpha\r\nkeep\n";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(HostId(2042));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut context, |context| active_id(&app, context).is_some());
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    std::fs::write(&path, format!("{original}external\n")).unwrap();
    let release = host.pause_next_read();
    app.update_in(&mut context, |app, window, cx| {
        app.editor_reload_from_disk(app.active_buffer().unwrap().id(), window, cx)
    });
    wait_until(&mut context, |_| host.entered.load(Ordering::SeqCst));
    view.update_in(&mut context, |view, window, cx| {
        view.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
    });
    release.send(()).unwrap();
    wait_until(&mut context, |context| {
        app.read_with(context, |app, _| {
            app.active_buffer().unwrap().conflict.is_some()
        })
    });
    view.read_with(&context, |view, cx| {
        assert!(view.document().read(cx).has_composition());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"nialpha\r\nkeep\n"
        );
    });
    view.update_in(&mut context, |view, window, cx| {
        view.replace_text_in_range(None, "你", window, cx)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn markdown_pristine_reload_allows_cursor_and_mode_changes_during_disk_read(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Focusable as _;
    let directory = canonical_tempdir();
    let path = directory.path().join("pristine-reload.md");
    let original = "alpha\r\n\r\nkeep\n";
    let loaded = "external\r\n\r\nupdated\n";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(HostId(2043));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut context, |context| active_id(&app, context).is_some());
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    let release = host.pause_next_read();
    std::fs::write(&path, loaded).unwrap();
    app.update_in(&mut context, |app, window, cx| {
        app.editor
            .buffers
            .rebuild_watch(Some(host.clone()), cx, |app| &mut app.editor.buffers);
        app.editor.buffers.external_change(
            &path,
            window,
            cx,
            |app| &mut app.editor.buffers,
            crate::ui::code_editor::EditorPanelState::buffer_edited,
        );
    });
    wait_until(&mut context, |_| host.entered.load(Ordering::SeqCst));
    view.update_in(&mut context, |view, _window, cx| {
        view.select_range(2..5, cx);
        view.toggle_mode(cx);
    });
    release.send(()).unwrap();
    wait_until(&mut context, |context| {
        view.read_with(context, |view, cx| {
            view.document().read(cx).encode().unwrap() == loaded.as_bytes()
        })
    });
    app.read_with(&context, |app, _| {
        let file = app.active_buffer().unwrap();
        assert!(file.conflict.is_none());
        assert!(!file.dirty);
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            loaded.as_bytes()
        )
    });
    assert_eq!(std::fs::read(&path).unwrap(), loaded.as_bytes());
}

#[gpui_kit::test]
fn markdown_clean_reload_survives_selection_and_group_handoff_without_overwriting_disk(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::input::SelectAll;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delayed-reload.mdx");
    std::fs::write(&path, b"alpha\r\nbeta\nlast").unwrap();
    let host = FileHost::new(HostId(230));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
    let first = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    start_active_markdown_in_source(&app, &mut vcx);
    let second = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });

    // The external-change path uses this same Host-bound reload. Pause its
    // read, then change only presentation state, not the document's bytes.
    let external = b"\xef\xbb\xbfexternal  \r\nsecond\nlast";
    std::fs::write(&path, external).unwrap();
    let release = host.pause_next_read();
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_reload_from_disk(app.active_buffer().unwrap().id(), window, cx);
    });
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_focus_group(true, window, cx));
    });
    vcx.dispatch_action(SelectAll);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert!(!app.active_buffer().unwrap().dirty);
        assert_eq!(first.read(cx).selected_range(), 0..15);
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .text()
                .unwrap()
                .read(cx)
                .text()
                .to_string()
                == "external  \nsecond\nlast"
        })
    });
    vcx.update(|_, cx| {
        assert_eq!(
            first.read(cx).document().entity_id(),
            second.read(cx).document().entity_id()
        );
        assert_eq!(
            first.read(cx).document().read(cx).encode().unwrap(),
            external
        );
        assert_eq!(
            second.read(cx).document().read(cx).encode().unwrap(),
            external
        );
    });
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), external);
    assert_eq!(host.writes.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
fn unchanged_text_refresh_preserves_selection_and_saved_undo_history(cx: &mut TestAppContext) {
    use gpui_kit::component::input::{MoveToStart, Redo, Undo};

    let dir = canonical_tempdir();
    let path = dir.path().join("unchanged.txt");
    std::fs::write(&path, "alpha\nbeta\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor_at(&path, Some(1), Some(1), window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().is_some_and(|file| file.path == path)
                && app.editor.buffers.watch.is_some()
        })
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("NEW161", cx);
    });
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), b"NEW161alpha\nbeta\n");
    vcx.dispatch_action(MoveToStart);
    vcx.update(|window, cx| {
        for _ in 0..3 {
            window.press("shift-right", cx);
        }
    });
    let selection = app.read_with(&vcx, |app, cx| {
        app.active_buffer().unwrap().input.text().unwrap().read(cx).selected_range()
    });
    assert_eq!(selection, 0..3);

    // A changed mtime requires a real Host read even though the bytes are
    // identical. Wait for that version to land, not a fixed timer delay.
    let modified = std::time::SystemTime::now() + Duration::from_secs(10);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let expected = MTime::from_system_time(std::fs::metadata(&path).unwrap().modified().unwrap());
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().unwrap().disk_mtime == Some(expected)
        })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(
            file.input.text().unwrap().read(cx).text().to_string(),
            "NEW161alpha\nbeta\n"
        );
        assert_eq!(file.input.text().unwrap().read(cx).selected_range(), selection);
        assert!(!file.dirty);
        assert!(file.conflict.is_none());
    });
    vcx.dispatch_action(Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "alpha\nbeta\n");
        assert!(file.dirty);
    });
    vcx.dispatch_action(Redo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(
            file.input.text().unwrap().read(cx).text().to_string(),
            "NEW161alpha\nbeta\n"
        );
        assert!(!file.dirty);
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
                .text()
                .unwrap()
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
fn adopting_a_workspace_discards_a_pending_open_without_losing_dirty_documents(
    cx: &mut TestAppContext,
) {
    use crate::core::session::{Session, WorkspaceStore};
    use crate::terminal::view::quiet_test_pane;
    use crate::ui::pane::{Pane, PaneSlot};
    use crate::ui::tab::Tab;

    let old_root = canonical_tempdir();
    let new_root = canonical_tempdir();
    let path = Path::new("/navigation.txt");
    std::fs::write(old_root.path().join("navigation.txt"), "old workspace\n").unwrap();
    std::fs::write(
        new_root.path().join("navigation.txt"),
        "new workspace\nsecond line\n",
    )
    .unwrap();
    let dirty_path = old_root.path().join("unsaved.txt");
    std::fs::write(&dirty_path, "keep me\n").unwrap();
    let older = FileHost::mapped(HostId(72), old_root.path());
    let newer = FileHost::mapped(HostId(73), new_root.path());
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&dirty_path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let dirty = active_id(&app, &vcx).unwrap();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("DIRTY161", cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor_unsaved() == vec![dirty])
    });
    let release = older.pause_next_read();
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host_at(older.clone(), path, Some(Position::new(0, 1)), window, cx);
    });
    wait_until(&mut vcx, |_| older.entered.load(Ordering::SeqCst));
    let next_workspace = app.update_in(&mut vcx, |app, window, cx| {
        let next = WorkspaceStore::claim(cx, None);
        assert_ne!(app.workspace, next);
        app.adopt_workspace(next, Session::default(), window, cx);
        // As in harness_with_tabs, provide a quiet terminal presentation, not
        // a shell or daemon, for the newly adopted empty workspace.
        let (view, stream) = quiet_test_pane(2, window, cx);
        app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        app.tabs.select(0);
        streams.push(stream);
        next
    });
    release.send(()).unwrap();
    // Invalidation itself clears pending, so it cannot acknowledge completion.
    // The request owns Host clones until both its worker and landing callback
    // finish; an installed stale buffer is also a completed, failing outcome.
    wait_until(&mut vcx, |vcx| {
        Arc::strong_count(&older) == 1
            || app.read_with(vcx, |app, _| {
                app.editor.buffers.at(older.id(), path).is_some()
            })
    });
    vcx.update(|window, cx| window.render_frame(cx));
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.workspace, next_workspace);
        assert!(
            app.editor.buffers.at(older.id(), path).is_none(),
            "the old navigation must not land"
        );
        assert!(
            app.tab_code().unwrap().files.contains(&dirty),
            "the new quiet tab adopts the dirty orphan"
        );
        let file = app.editor.buffers.get(dirty).unwrap();
        assert_eq!(file.path, dirty_path);
        assert!(file.dirty);
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "DIRTY161keep me\n");
    });
    // Open only after the old callback has finished: a newer request must not
    // be what invalidates the stale request on behalf of the transition.
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host_at(newer, path, Some(Position::new(1, 4)), window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, location)| host == HostId(73) && location == path)
        })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.host.id(), HostId(73));
        assert_eq!(
            file.input.text().unwrap().read(cx).text().to_string(),
            "new workspace\nsecond line\n"
        );
        assert_eq!(file.input.text().unwrap().read(cx).cursor_position(), Position::new(1, 4));
        let files = &app.tab_code().unwrap().files;
        assert!(files.contains(&dirty));
        assert!(files.contains(&file.id()));
        assert!(app.editor.buffers.at(older.id(), path).is_none());
        assert_eq!(app.editor_unsaved(), vec![dirty]);
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&dirty_path, window, cx);
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), dirty);
        assert!(file.dirty);
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "DIRTY161keep me\n");
    });
}

#[gpui_kit::test]
fn rebinding_a_host_discards_a_pending_open_without_losing_dirty_documents(
    cx: &mut TestAppContext,
) {
    use crate::ui::host_registry::HostRegistry;

    let old_root = canonical_tempdir();
    let new_root = canonical_tempdir();
    let path = Path::new("/navigation.txt");
    std::fs::write(old_root.path().join("navigation.txt"), "old connection\n").unwrap();
    std::fs::write(
        new_root.path().join("navigation.txt"),
        "new connection\nsecond line\n",
    )
    .unwrap();
    let dirty_path = old_root.path().join("unsaved.txt");
    std::fs::write(&dirty_path, "keep me\n").unwrap();
    let older = FileHost::mapped(HostId::LOCAL, old_root.path());
    let newer = FileHost::mapped(HostId::LOCAL, new_root.path());
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&dirty_path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let dirty = active_id(&app, &vcx).unwrap();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("DIRTY161", cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor_unsaved() == vec![dirty])
    });
    let release = older.pause_next_read();
    let context = app.update_in(&mut vcx, |app, window, cx| {
        HostRegistry::insert(cx, older.clone());
        app.open_file_in_editor_at(path, Some(1), Some(2), window, cx);
        (app.workspace, app.tabs[0].id())
    });
    wait_until(&mut vcx, |_| older.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, _, cx| {
        let previous = app.spawn_host(cx);
        // A reconnect replaces the adapter without changing machine identity.
        // Keep the same workspace/tab too, so target identity guards alone
        // cannot stand in for rebind_host retiring its in-flight requests.
        HostRegistry::insert(cx, newer.clone());
        app.rebind_host(previous, cx);
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        Arc::strong_count(&older) == 1
            || app.read_with(vcx, |app, _| {
                app.editor.buffers.at(older.id(), path).is_some()
            })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!((app.workspace, app.tabs[0].id()), context);
        assert_eq!(app.tab_code().unwrap().files.len(), 1);
        assert!(app.editor.buffers.at(older.id(), path).is_none());
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), dirty);
        assert!(file.dirty);
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "DIRTY161keep me\n");
    });
    // Do not issue a replacement open until the stale request has settled.
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor_at(path, Some(2), Some(5), window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().is_some_and(|file| file.id() != dirty)
        })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.host.id(), HostId::LOCAL);
        assert_eq!(file.path, path);
        assert_eq!(
            file.input.text().unwrap().read(cx).text().to_string(),
            "new connection\nsecond line\n"
        );
        assert_eq!(file.input.text().unwrap().read(cx).cursor_position(), Position::new(1, 4));
        assert_eq!(app.tab_code().unwrap().files.len(), 2);
        assert_eq!(app.editor_unsaved(), vec![dirty]);
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&dirty_path, window, cx);
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), dirty);
        assert!(file.dirty);
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "DIRTY161keep me\n");
    });
}

#[gpui_kit::test]
fn markdown_session_restore_surfaces_failed_reads_without_losing_valid_files(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("restored.md");
    let missing = dir.path().join("unavailable.mdx");
    std::fs::write(&valid, "# restored\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        app.tab_code_mut_or_init();
        app.editor.restored.remove(&tab);
        editor_session::put(
            cx,
            tab,
            TabEditor {
                files: vec![
                    dir.path().join("unavailable-markdown.md"),
                    missing.clone(),
                    valid.clone(),
                ],
                active: 2,
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
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "# restored\n");
        assert!(file.input.code().is_none());
        assert_eq!(app.tab_code().unwrap().files.len(), 1);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |_, cx| {
            let history = crate::ui::notification_history::history(cx);
            let history = history.read(cx);
            ["unavailable-markdown.md", "unavailable.mdx"]
                .iter()
                .all(|name| {
                    history
                        .records()
                        .any(|record| record.message().contains(*name))
                })
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
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    start_active_markdown_in_source(&app, &mut vcx);
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
                .text()
                .unwrap()
                .read(cx)
                .text()
                .to_string(),
            "  first\nsecond\n"
        )
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    start_active_markdown_in_source(&app, &mut vcx);
    vcx.update(|window, cx| window.render_frame(cx));
    app.update_in(&mut vcx, |app, window, cx| {
        app.active_editor_input()
            .unwrap()
            .text()
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
                .text()
                .unwrap()
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
                .text()
                .unwrap()
                .read(cx)
                .text()
                .to_string(),
            "  first\nsecond\n"
        )
    });
}
