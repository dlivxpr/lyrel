use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::view::test_support::*;
use gpui_kit::TestAppContext;
use rio_vt::crosswords::pos::{Column, Line};

    #[gpui_kit::test]
    fn a_relative_path_links_against_the_panes_own_directory(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lyrel-view-link-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("scratchpad")).expect("create scratchpad dir");
        let notes = dir.join("scratchpad/notes.md");
        std::fs::write(&notes, b"# notes").expect("create notes.md");

        let (window, mut daemon) = harness(cx);
        DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
        DaemonMsg::Output(b"ready (scratchpad/notes.md) and (scratchpad/gone.md)\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| {
                    view.cwd().is_some()
                        && view.terminal.term.lock().grid[Line(0)][Column(7)].c() == 's'
                })
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.cwd().as_deref(), Some(dir.as_path()));
                assert!(
                    view.hover_link_at(7, 0, true, cx),
                    "the file is right there under the pane's directory"
                );
                assert!(
                    view.links.hovered().is_some_and(|link| link.is_armed()),
                    "with the modifier down it is ready to be clicked"
                );
                assert!(
                    view.hover_link_at(7, 0, false, cx),
                    "a path is pointed out before the modifier is down, not after"
                );
                assert!(
                    view.links.hovered().is_some_and(|link| !link.is_armed()),
                    "but held back, because a plain click will not follow it"
                );

                let gone = "ready (scratchpad/notes.md) and (".len();
                assert!(
                    !view.hover_link_at(gone, 0, true, cx),
                    "nothing underlines for a name that is not on disk"
                );
                match view.resolve_link_at(gone, 0, true, false, cx) {
                    LinkAt::Unresolved { candidate, pending } => {
                        assert_eq!(candidate.path, "scratchpad/gone.md");
                        assert!(
                            candidate.looks_like_a_path(view.links.roots().style),
                            "so the click reports it instead of staying silent"
                        );
                        assert!(!pending, "a local pane answers on the spot");
                    }
                    _ => panic!("expected an unresolved path-shaped candidate"),
                }

                view.record_menu_link(7, 0, cx);
                assert!(
                    matches!(
                        view.menu_link_path(),
                        Some(path) if path.ends_with("scratchpad/notes.md")
                    ),
                    "a right click over a path opens a menu about that path"
                );
                view.record_menu_link(0, 0, cx);
                assert!(
                    view.menu_link_path().is_none(),
                    "and a right click over `ready` opens the ordinary one"
                );

                let mut off = cx.global::<Config>().clone();
                off.link_url = false;
                cx.set_global(off);
                view.record_menu_link(7, 0, cx);
                assert!(
                    view.menu_link_path().is_none(),
                    "and with link detection turned off the menu offers nothing \
                     the underline and the click both refuse"
                );
                cx.set_global(Config::default());

                // `ready (scratchpad...`: the blank between the two words.
                assert!(!view.hover_link_at(5, 0, false, cx));
                assert!(view.links.hovered().is_none(), "a blank holds no link");
                assert_eq!(
                    view.links.last_cell(),
                    Some((5, 0)),
                    "and the pointer is still remembered, so crossing a run \
                     of blanks costs one look each rather than one a frame"
                );
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Absolute paths in a pane that is `ssh`-ed somewhere used to be resolved
    /// against *this* machine's filesystem, so `/etc/hosts` on the far end
    /// opened the local copy without a word about it.
    #[gpui_kit::test]
    fn a_pane_whose_paths_are_elsewhere_does_not_link_local_files(cx: &mut TestAppContext) {
        let file = std::env::temp_dir().join(format!("lyrel-elsewhere-{}.txt", std::process::id()));
        std::fs::write(&file, b"local").expect("create local file");
        let line = format!("open {} now\r\n", file.display());

        let (window, mut daemon) = harness(cx);
        DaemonMsg::Output(line.clone().into_bytes())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::RemoteContext(Some(crate::daemon::protocol::RemoteContext {
            kind: crate::daemon::protocol::RemoteKind::Ssh,
            argv: vec!["ssh".into(), "box".into()],
            target: "box".into(),
        }))
        .encode(&mut daemon)
        .unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| view.remote_context().is_some())
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                assert!(
                    !view.cwd_is_on_host(),
                    "a local host cannot answer for the far side of an ssh session"
                );
                assert!(
                    !view.hover_link_at(6, 0, true, cx),
                    "the local copy of that path is not what the pane printed"
                );
            })
            .unwrap();
        let _ = std::fs::remove_file(&file);
    }

    /// A path a remote pane printed stays *wanted* until a host has actually
    /// been asked about it.
    ///
    /// `take_wanted` moves paths into the in-flight set on the promise that a
    /// call is carrying them; a lookup that finds no host to make that call
    /// must not have made the promise. Otherwise those paths sit "not answered
    /// yet" for the life of the pane — no underline, and a click that says
    /// nothing, which is the silence this whole path exists to remove.
    ///
    /// Was unix-only because the path it prints is: `Path::new("/etc/hosts")`
    /// is not absolute on Windows, so `FileCandidate::paths` measured it from
    /// the roots rather than letting it stand alone — and a workspace that
    /// never connected has no roots, so nothing was ever wanted. Which was
    /// itself the divergence: a Windows lyrel looking at a *remote* Linux pane
    /// never probed the POSIX paths that pane printed.
    ///
    /// #795 settled that. `paths` now asks the pane's own
    /// [`super::search::PathStyle`] rather than this machine's, and a remote
    /// pane that has not reported a cwd is read as `Posix`, so `/etc/hosts`
    /// stands alone on every client. The gate is only still here because
    /// nothing has run this test on Windows yet; lifting it belongs in a
    /// change that can show it green, not in a merge.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn a_probe_with_no_host_to_ask_stays_wanted(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Output(b"see /etc/hosts now\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| {
                    view.terminal.term.lock().grid[Line(0)][Column(4)].c() == '/'
                })
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);
                assert!(
                    view.host(cx).is_none(),
                    "a workspace that never connected has nothing to ask"
                );
                assert!(
                    view.cwd_is_on_host(),
                    "the pane's paths are the far side's, whether or not it is up"
                );
                assert!(
                    !view.hover_link_at(4, 0, true, cx),
                    "nothing underlines while the answer is still unknown"
                );
                assert_eq!(
                    view.links.take_probes().1,
                    vec![std::path::PathBuf::from("/etc/hosts")],
                    "and the question survives, ready for a host to answer it"
                );
                assert!(
                    !view.links.roots().local_home,
                    "this machine's $HOME describes nobody over there"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_stale_hover_row_does_not_index_the_shrunken_grid(cx: &mut TestAppContext) {
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                view.hover_link_at(0, 23, true, cx);
                view.terminal.resize(TermSize::new(80, 8), 8, 17);
                assert!(
                    !view.refresh_link_hover(true, cx),
                    "a row outside the grid can't hold a link"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_resize_forgets_the_hovered_cell(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Output(b"\x1b[24;1Hhttps://example.com".to_vec()).encode(&mut daemon).unwrap();
        for _ in 0..200 {
            if window.update(cx, |view, _, _| {
                view.terminal.term.lock().grid[Line(23)][Column(0)].c() == 'h'
            }).unwrap() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, cx| {
                view.set_grid_size(80, 24, px(8.), px(17.), 1., cx);
                view.hover_link_at(0, 23, true, cx);
                assert_eq!(view.links.last_cell(), Some((0, 23)));
                assert!(view.links.hovered().is_some(), "the printed URL is underlined");
                view.set_grid_size(80, 24, px(8.), px(17.), 1., cx);
                assert_eq!(view.links.last_cell(), Some((0, 23)));
                view.set_grid_size(80, 8, px(8.), px(17.), 1., cx);
                assert!(view.links.last_cell().is_none(), "the cell is stale");
                assert!(view.links.hovered().is_none(), "so is the link it resolved");
            })
            .unwrap();
    }

    /// The seam is invisible to the user, so it has to be invisible to the
    /// hover too: pointing at either half underlines the whole path, and the
    /// span the element paints reaches across both rows.
    #[gpui_kit::test]
    fn a_path_the_terminal_wrapped_is_hovered_as_one_link(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lyrel-view-wrap-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/bb/ccc/dddd")).expect("create dirs");
        std::fs::write(dir.join("a/bb/ccc/dddd/notes.md"), b"# notes").expect("create notes.md");

        let (window, mut daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                view.set_grid_size(20, 6, px(8.), px(17.), 1., cx);
            })
            .unwrap();
        DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
        DaemonMsg::Output(b"see a/bb/ccc/dddd/notes.md here\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| {
                    view.cwd().is_some()
                        && view.terminal.term.lock().grid[Line(1)][Column(0)].c() == 'e'
                })
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                // Row 0 holds `see a/bb/ccc/dddd/n`, row 1 the rest.
                for (col, row, where_) in [(6, 0, "before the seam"), (2, 1, "after it")] {
                    assert!(
                        view.hover_link_at(col, row, true, cx),
                        "the wrapped path is a link from {where_}"
                    );
                    let link = view.links.hovered().expect("a span");
                    let (start, end) = (link.runs()[0].0, link.runs()[link.runs().len() - 1].1);
                    assert_eq!(
                        (start.row, end.row),
                        (Line(0), Line(1)),
                        "and the span the element paints covers both rows"
                    );
                    assert_eq!(start.col, Column(4), "starting at the path itself");
                }
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A wide character owns two columns, and the second one holds a space.
    /// The hover has to read that as part of the glyph, or the underline goes
    /// out on every other column of a path written in CJK.
    #[gpui_kit::test]
    fn the_second_column_of_a_wide_character_still_hovers(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lyrel-view-wide-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("文档")).expect("create dirs");
        std::fs::write(dir.join("文档/笔记.md"), b"# notes").expect("create notes");

        let (window, mut daemon) = harness(cx);
        DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
        DaemonMsg::Output("see 文档/笔记.md here\r\n".as_bytes().to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| {
                    view.cwd().is_some()
                        && view.terminal.term.lock().grid[Line(0)][Column(4)].c() == '文'
                })
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                // `see 文档/…`: column 4 carries 文, column 5 is its spacer.
                assert!(!crate::terminal::link::cell_is_blank(&view.terminal.term, 5, 0), "the spacer belongs to the glyph");
                for col in [4, 5] {
                    assert!(
                        view.hover_link_at(col, 0, true, cx),
                        "column {col} of the same character is the same link"
                    );
                }
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A full-screen application drew the grid and is watching the mouse
    /// itself, so lyrel stays out of it until asked.
    #[gpui_kit::test]
    fn a_path_under_a_full_screen_application_waits_for_the_modifier(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lyrel-view-alt-link-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("scratchpad")).expect("create scratchpad dir");
        std::fs::write(dir.join("scratchpad/notes.md"), b"# notes").expect("create notes.md");

        let (window, mut daemon) = harness(cx);
        DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
        DaemonMsg::Output(b"\x1b[?1049hready scratchpad/notes.md\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| {
                    view.cwd().is_some()
                        && view.on_alt_screen()
                        && view.terminal.term.lock().grid[Line(0)][Column(6)].c() == 's'
                })
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                assert!(view.on_alt_screen(), "the application took the grid");
                assert!(
                    !view.hover_link_at(6, 0, false, cx),
                    "nothing is pointed out over somebody else's window"
                );
                assert!(
                    view.hover_link_at(6, 0, true, cx),
                    "asking for lyrel's reading of the screen still gets it"
                );
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_menu_is_headed_by_the_file_it_is_about() {
        assert_eq!(
            link_menu_label(std::path::Path::new("/a/very/long/way/down/notes.md")),
            "notes.md",
            "the name, so the menu is not as wide as the path"
        );
        assert_eq!(link_menu_label(std::path::Path::new("/")), "/");
    }


#[gpui_kit::test]
fn a_link_menu_keeps_its_file_and_navigation_position_after_the_pointer_moves(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.md"), "one\ntwo\nthree\n").unwrap();
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    let (window, mut daemon) = harness(cx);
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = opened.clone();
    window.update(cx, |_, _, cx| {
        let view = cx.entity();
        cx.subscribe(&view, move |_, _, event: &OpenFileRequested, _| {
            seen.borrow_mut().push((event.host_id, event.path.clone(), event.line, event.column, event.is_dir));
        }).detach();
    }).unwrap();
    DaemonMsg::Cwd(dir.path().to_path_buf()).encode(&mut daemon).unwrap();
    DaemonMsg::Output(b"notes.md:3:2 folder\r\n".to_vec()).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        if window.update(cx, |view, _, _| {
            view.cwd().is_some() && view.terminal.term.lock().grid[Line(0)][Column(13)].c() == 'f'
        }).unwrap() { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window.update(cx, |view, window, cx| {
        view.record_menu_link(0, 0, cx);
        assert!(view.hover_link_at(13, 0, true, cx));
        view.copy_menu_link_path(cx);
        view.open_menu_link(window, cx);
    }).unwrap();
    cx.run_until_parked();
    assert_eq!(&*opened.borrow(), &[(crate::ui::host_ops::HostId::LOCAL,
        dir.path().join("notes.md"), Some(3), Some(2), false)]);
    assert_eq!(cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(dir.path().join("notes.md").to_string_lossy().into_owned()));
    window.update(cx, |view, window, cx| {
        view.record_menu_link(13, 0, cx);
        view.hover_link_at(0, 0, true, cx);
        view.open_menu_link(window, cx);
    }).unwrap();
    cx.run_until_parked();
    assert_eq!(opened.borrow().last(), Some(&(crate::ui::host_ops::HostId::LOCAL,
        dir.path().join("folder"), None, None, true)));
}

#[gpui_kit::test]
fn late_link_answers_cannot_retarget_a_changed_host_directory_or_grid(cx: &mut TestAppContext) {
    use crate::terminal::link_probe::Existence;
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Output(b"/project/notes.rs:3:2\r\n".to_vec()).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        if window.update(cx, |view, _, _| {
            view.terminal.term.lock().grid[Line(0)][Column(1)].c() == 'p'
        }).unwrap() { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window.update(cx, |view, _, cx| {
        bind_to_a_disconnected_remote_workspace(view, cx);
        view.set_git_status_cwd_for_test(Some("/project".into()));
        assert!(!view.hover_link_at(1, 0, true, cx));
        let (epoch, wanted) = view.links.take_probes();
        assert!(wanted.contains(&std::path::PathBuf::from("/project/notes.rs")));
        let answers = vec![("/project/notes.rs".into(), Existence::File)];
        let (root_epoch, _) = view.links.request_root().unwrap();
        view.hover_link_at(60, 0, true, cx);
        assert!(view.links.land_probes(epoch, answers.clone()));
        assert!(!view.recompute_link_hover(cx), "a superseded target cannot regain the underline");
        view.set_git_status_cwd_for_test(Some("/elsewhere".into()));
        view.sync_link_location();
        assert!(!view.links.land_probes(epoch, answers.clone()));
        assert!(!view.links.land_root(root_epoch, Some("/old-root".into())));
        assert_eq!(view.links.roots().dirs, vec![std::path::PathBuf::from("/elsewhere")]);
        assert!(!view.hover_link_at(1, 0, true, cx));
        let (epoch, _) = view.links.take_probes();
        view.set_grid_size(80, 8, px(8.), px(17.), 1., cx);
        assert!(!view.links.land_probes(epoch, answers.clone()));
        assert!(!view.hover_link_at(1, 0, true, cx));
        let (epoch, _) = view.links.take_probes();
        view.set_workspace(None);
        view.sync_link_location();
        assert!(!view.links.land_probes(epoch, answers));
        assert!(view.links.hovered().is_none());
    }).unwrap();
}

#[gpui_kit::test]
fn a_remote_menu_keeps_its_host_and_internal_open_after_the_pane_retargets(cx: &mut TestAppContext) {
    use crate::terminal::link_probe::Existence;

    let (window, mut daemon) = harness(cx);
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = opened.clone();
    window.update(cx, |_, _, cx| {
        let view = cx.entity();
        cx.subscribe(&view, move |_, _, event: &OpenFileRequested, _| {
            seen.borrow_mut().push((event.host_id, event.path.clone(), event.line, event.column));
        }).detach();
    }).unwrap();
    DaemonMsg::Output(b"/project/notes.rs:3:2\r\n".to_vec()).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        if window.update(cx, |view, _, _| {
            view.terminal.term.lock().grid[Line(0)][Column(1)].c() == 'p'
        }).unwrap() { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let host = window.update(cx, |view, window, cx| {
        bind_to_a_disconnected_remote_workspace(view, cx);
        view.set_git_status_cwd_for_test(Some("/project".into()));
        assert!(!view.hover_link_at(1, 0, true, cx));
        let (epoch, wanted) = view.links.take_probes();
        assert!(wanted.contains(&std::path::PathBuf::from("/project/notes.rs")));
        assert!(view.links.land_probes(epoch, vec![("/project/notes.rs".into(), Existence::File)]));
        assert!(view.recompute_link_hover(cx));
        view.record_menu_link(1, 0, cx);
        let host = view.links.menu().expect("captured remote file").host();
        assert!(!host.is_local());

        // External-open preferences belong to local files, even when the pane
        // has changed Host by the time the captured menu action is dispatched.
        view.set_workspace(None);
        view.sync_link_location();
        for mode in [LinkFileOpen::System, LinkFileOpen::Command] {
            let mut config = cx.global::<Config>().clone();
            config.link_file_open = Some(mode);
            config.link_file_command = Some("lyrel-no-such-link-opener {path}".into());
            cx.set_global(config);
            view.open_menu_link(window, cx);
        }
        host
    }).unwrap();
    cx.run_until_parked();
    assert_eq!(&*opened.borrow(), &[
        (host, "/project/notes.rs".into(), Some(3), Some(2)),
        (host, "/project/notes.rs".into(), Some(3), Some(2)),
    ]);
}
