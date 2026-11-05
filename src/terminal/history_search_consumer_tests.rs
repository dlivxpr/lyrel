use super::*;

// Command bytes captured from zsh 5.9 using EXTENDED_HISTORY and fc -W;
// only the timestamp is fixed. Do not synthesize the escapes under test.
const METAFIED_ZSH_HISTORY: &[u8] = b": 1700000000:0;echo '\xe8\xaf\xbb\xe5\x83\xaf\x83\xb6\xe4\xbb\xa4\xe7\x83\xa9\x83\xac\xe5\xa4\xb1\xe8\xb4\xa5\xe3\x83\xa3\xbc\xf0\x83\xbf\x83\xb9\x82'\n";

// Captured separately from the same zsh 5.9 with EXTENDED_HISTORY disabled.
const PLAIN_METAFIED_ZSH_HISTORY: &[u8] = b"echo '\xe8\xaf\xbb\xe5\x83\xaf\x83\xb6\xe4\xbb\xa4\xe7\x83\xa9\x83\xac\xe5\xa4\xb1\xe8\xb4\xa5\xe3\x83\xa3\xbc\xf0\x83\xbf\x83\xb9\x82'\n";

#[gpui_kit::test]
fn metafied_zsh_history_is_searchable_and_submits_the_original_command(cx: &mut TestAppContext) {
    let (window, view, mut daemon) = rooted_harness(cx);
    let bytes = METAFIED_ZSH_HISTORY;
    let command = "echo '读取令牌失败ー🙂'";
    let loaded = crate::terminal::history::load_with_shell_files(
        &crate::terminal::history::Scope::remote("metafied-zsh-consumer"),
        vec![(".zsh_history".into(), bytes.to_vec())],
    );
    assert_eq!(loaded.entries, [command]);
    assert_eq!(
        loaded.meta.get(command),
        Some(&crate::terminal::history::EntryMeta {
            ts: Some(1_700_000_000),
            exit: None,
        }),
        "zsh elapsed seconds are not an exit status"
    );
    view.update(cx, |view, cx| {
        view.history = TerminalHistory::from_loaded(loaded);
        view.handle_editor_key(&key("ctrl-r"), cx);
        view.commit_text("echo", cx);
        assert_eq!(
            view.history
                .search()
                .and_then(|search| search.selected_line(view.history.entries())),
            Some(command)
        );
        view.handle_editor_key(&key("enter"), cx);
        assert_eq!(view.input.editor().text(), command);
        view.handle_editor_key(&key("ctrl-r"), cx);
        cx.notify();
    });
    window.update(cx, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.handle_editor_key(&key("cmd-enter"), cx);
        assert!(view.history.search().is_none());
        assert!(view.input.editor().is_empty());
    });
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(format!("{command}\r").into_bytes()),
        "history submission must preserve the original UTF-8 command exactly"
    );
}

/// Exercise the window keymap, not just the prompt editor's key handler. On a
/// Windows runner the Mac key/focus policy is modeled explicitly; this is not
/// native macOS acceptance.
#[gpui_kit::test]
fn reverse_search_cmd_enter_owns_dispatch_only_while_searching(cx: &mut TestAppContext) {
    use crate::core::config::KeybindingOverride;
    use gpui_kit::{OperatingSystem, PlatformInfo, PlatformTarget};

    cx.set_platform_info(PlatformInfo {
        os: OperatingSystem::Mac,
        target: PlatformTarget::Native,
    });
    let (app, mut vcx, mut daemon) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.update(|_, cx| {
        // Reproduce the Mac default on every build target, through the same
        // config/rebind path users use rather than a synthetic action handler.
        cx.global_mut::<Config>().keybindings.insert(
            "ToggleFullscreen".into(),
            KeybindingOverride::Exact(vec!["cmd-enter".into()]),
        );
        crate::ui::keymap::rebind(cx);
    });
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0]
            .pane
            .first_leaf()
            .unwrap()
            .terminal()
            .unwrap()
            .clone();
        pane.update(cx, |view, cx| {
            seed_history(view, vec!["echo SELECTED".into(), "echo OTHER".into()]);
            window.focus(&view.focus_handle, cx);
        });
        pane
    });
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        vcx.run_until_parked();
        if pane.read_with(&vcx, |view, _| view.input_active()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pane.read_with(&vcx, |view, _| view.input_active()));

    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    vcx.update(|window, _| {
        assert!(
            window.is_fullscreen(),
            "outside search fullscreen still wins"
        )
    });
    assert_eq!(next_input_until_timeout(&mut daemon), None);
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    vcx.update(|window, _| assert!(!window.is_fullscreen()));

    vcx.simulate_keystrokes("ctrl-r");
    vcx.simulate_input("SELECTED");
    vcx.run_until_parked();
    pane.read_with(&vcx, |view, _| {
        assert_eq!(
            view.history
                .search()
                .and_then(|search| search.selected_line(view.history.entries())),
            Some("echo SELECTED")
        );
    });
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    vcx.update(|window, _| {
        assert!(
            !window.is_fullscreen(),
            "reverse-search Cmd+Enter must not toggle fullscreen"
        );
    });
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"echo SELECTED\r".to_vec())
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "submit exactly once"
    );
    pane.read_with(&vcx, |view, _| {
        assert!(view.history.search().is_none());
        assert!(view.input.editor().is_empty());
    });

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        vcx.run_until_parked();
        if pane.read_with(&vcx, |view, _| view.input_active()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pane.read_with(&vcx, |view, _| view.input_active()));
    vcx.simulate_input("DRAFT_MUST_NOT_RUN");
    vcx.simulate_keystrokes("ctrl-r");
    vcx.simulate_input("NO_MATCH_FOR_THIS_QUERY");
    vcx.run_until_parked();
    assert!(pane.read_with(&vcx, |view, _| {
        view.history.search().unwrap().matches().is_empty()
    }));
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "no selection cannot submit the draft"
    );
    pane.read_with(&vcx, |view, _| {
        assert!(view.history.search().is_none());
        assert_eq!(view.input.editor().text(), "DRAFT_MUST_NOT_RUN");
    });

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("editor.txt");
    std::fs::write(&path, "EDITOR").unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    for _ in 0..200 {
        vcx.run_until_parked();
        if app.read_with(&vcx, |app, _| app.editor_active_local_file().is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.read_with(&vcx, |app, _| app.editor_active_local_file().is_some()));
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.editor_active_local_file()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "EDITOR\n"
        );
    });
    vcx.update(|window, _| assert!(!window.is_fullscreen(), "the editor keeps InsertLineBelow"));
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[cfg(windows)]
#[gpui_kit::test]
fn reverse_search_cmd_enter_does_not_add_a_windows_binding(cx: &mut TestAppContext) {
    let (app, mut vcx, mut daemon) = crate::ui::app::test_window::harness_with_pane(cx);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.terminals()[0].clone();
        pane.update(cx, |view, cx| {
            seed_history(view, vec!["echo WINDOWS_SELECTED".into()]);
            window.focus(&view.focus_handle, cx);
        });
        pane
    });
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        vcx.run_until_parked();
        if pane.read_with(&vcx, |view, _| view.input_active()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pane.read_with(&vcx, |view, _| view.input_active()));
    vcx.simulate_keystrokes("ctrl-r cmd-enter");
    vcx.run_until_parked();
    assert!(pane.read_with(&vcx, |view, _| view.history.search().is_some()));
    assert_eq!(next_input_until_timeout(&mut daemon), None);
    vcx.simulate_keystrokes("ctrl-enter");
    vcx.run_until_parked();
    pane.read_with(&vcx, |view, _| {
        assert!(view.history.search().is_none());
        assert_eq!(view.input.editor().text(), "echo WINDOWS_SELECTED");
    });
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "Ctrl+Enter only accepts on Windows"
    );
}

#[gpui_kit::test]
fn ctrl_r_fuzzy_search_accepts_into_the_editor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "cargo build", "git commit -m x"]
                .into_iter()
                .map(String::from)
                .collect());
            

            view.handle_editor_key(&key("ctrl-r"), cx);
            assert!(view.history.search().is_some(), "Ctrl+R opens the search");
            view.commit_text("gst", cx);
            assert_eq!(
                view.history.search()
                    .as_ref()
                    .and_then(|rs| rs.selected_line(view.history.entries())),
                Some("git status")
            );
            view.handle_editor_key(&key("enter"), cx);
            assert!(view.history.search().is_none(), "Enter closes the search");
            assert_eq!(view.input.editor().text(), "git status");
        })
        .unwrap();
}

#[gpui_kit::test]
fn ctrl_r_steps_matches_and_cmd_enter_runs(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();

    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "cargo build", "git commit -m x"]
                .into_iter()
                .map(String::from)
                .collect());
            

            view.handle_editor_key(&key("ctrl-r"), cx);
            view.commit_text("git", cx);
            assert_eq!(
                view.history.search()
                    .as_ref()
                    .and_then(|rs| rs.selected_line(view.history.entries())),
                Some("git commit -m x")
            );
            view.handle_editor_key(&key("ctrl-r"), cx);
            assert_eq!(
                view.history.search()
                    .as_ref()
                    .and_then(|rs| rs.selected_line(view.history.entries())),
                Some("git status")
            );
            view.handle_editor_key(&key("cmd-enter"), cx);
            assert!(view.history.search().is_none());
            assert!(view.input.editor().is_empty(), "submit clears the editor");
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"git status\r".to_vec()),
        "Cmd+Enter ships the selected line to the PTY"
    );
}

#[gpui_kit::test]
fn reverse_search_menu_survives_a_real_render_pass(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.at_prompt())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "prompt report engages the editor");
            let mut history = history_fixture(["git status", "cargo build --release", "echo hello"]
                .into_iter()
                .map(String::from)
                .collect());
            
            history.meta.insert(
                "cargo build --release".into(),
                crate::terminal::history::EntryMeta {
                    ts: Some(unix_now().saturating_sub(7200)),
                    exit: Some(1),
                },
            );
            view.history = TerminalHistory::from_loaded(history);
            view.handle_editor_key(&key("ctrl-r"), cx);
            view.commit_text("c", cx);
            assert!(
                view.history.search()
                    .as_ref()
                    .is_some_and(|rs| !rs.matches().is_empty()),
                "the query has matches for the menu to draw"
            );
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.history.search().is_some(), "search survives the frame");
        })
        .unwrap();
}
    /// #541: the history menu floats over live grid cells, and a `div` that
    /// carries no handler of its own inserts no hitbox — so the press went
    /// straight through to the grid behind the menu, which cleared the
    /// selection, dragged out a new one, or (with the link modifier held)
    /// opened whatever link the menu was covering.
    ///
    /// The second half is the other side of the pair `src/ui/app.rs` keeps for
    /// the resize handle: with the menu gone the very same press must still
    /// reach the grid, or this would pass on a pane that never sees a mouse.
    #[gpui_kit::test]
    fn a_press_on_the_history_menu_never_reaches_the_grid(cx: &mut TestAppContext) {
        use gpui_kit::{MouseMoveEvent, PlatformInput};

        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        prompt_ready(&window, cx, &mut daemon);
        wait_for_input_active(&window, cx);

        window
            .update(cx, |view, window, cx| {
                window.activate_window();
                view.focus_handle.focus(window, cx);
                seed_history(view, vec!["echo one".to_string(), "echo two".to_string()]);
                
                view.handle_editor_key(&key("ctrl-r"), cx);
                cx.notify();
            })
            .unwrap();

        let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();

        // The menu is laid out one row under the cursor, plus the gap
        // `render_reverse_search_menu` leaves; this lands in the middle of its
        // first row, where a candidate is drawn.
        let lh = window.update(cx, |view, _, _| view.line_height).unwrap();
        let at = point(
            px(GRID_PAD_X) + px(10.),
            px(GRID_PAD_Y) + lh * 2.0 + px(10.),
        );

        let press = |vcx: &mut gpui_kit::VisualTestContext| {
            vcx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::none(),
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: at,
                        modifiers: Modifiers::none(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
            });
            vcx.run_until_parked();
        };

        press(&mut vcx);
        window
            .update(cx, |view, _, _| {
                assert!(
                    view.history.search().is_some(),
                    "the press must not have closed the menu either"
                );
                assert!(
                    !view.selection_scroll.is_selecting(),
                    "the menu swallowed the press, so no selection started under it"
                );
            })
            .unwrap();

        window
            .update(cx, |view, _, cx| {
                view.history.cancel_search();
                cx.notify();
            })
            .unwrap();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();

        press(&mut vcx);
        window
            .update(cx, |view, _, _| {
                assert!(
                    view.selection_scroll.is_selecting(),
                    "with no menu over it the same press is the grid's to take"
                );
            })
            .unwrap();
    }

#[gpui_kit::test]
fn custom_histfile_seed_reaches_the_owning_panes_reverse_search(cx: &mut TestAppContext) {
    // Other consumers persist an application history file at the pinned path;
    // this consumer needs the daemon's per-pane directory at that path. Use an
    // isolated process instead of touching another test's history fixture.
    if std::env::var_os("LYREL_CUSTOM_HISTORY_CONSUMER_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{}::custom_histfile_seed_reaches_the_owning_panes_reverse_search", module_path!().split_once("::").unwrap().1)])
            .args(["--nocapture", "--test-threads=1", "--"])
            .env("LYREL_CUSTOM_HISTORY_CONSUMER_CHILD", "1")
            .output().unwrap();
        assert!(output.status.success(), "isolated production history consumer failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (_window, view, mut daemon) = rooted_harness(cx);
    let pane = 184_001;
    let path = crate::daemon::history::path_for(pane).unwrap();
    let other = crate::daemon::history::path_for(pane + 1).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let origin = tempfile::TempDir::new().unwrap();
    let custom = origin.path().join("custom history");
    let seed = [b"echo custom_histfile_seed_184\n".as_slice(), PLAIN_METAFIED_ZSH_HISTORY].concat();
    std::fs::write(&custom, &seed).unwrap();
    std::fs::write(&other, ": 1700000000:0;echo other_panes_private_seed_184\n").unwrap();
    cx.update(|cx| {
        let config = cx.global_mut::<Config>();
        config.per_pane_history = true;
        config.shell = Some(crate::core::config::ShellConfig { program: "/usr/bin/zsh".into(), args: Vec::new() });
    });
    view.update(cx, |view, cx| {
        view.pane_id = pane;
        // No pane override: the existing local default resolver knows zsh.
        view.shell_spec = None;
        view.follow_history_scope(cx);
    });
    cx.run_until_parked();
    // The shell creates its private file after the view is constructed, once
    // the user's rc has selected HISTFILE. The real zsh journey covers seeding.
    std::fs::copy(&custom, &path).unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }.encode(&mut daemon).unwrap();
    for _ in 0..200 {
        if view.read_with(cx, |view, _| view.terminal.at_prompt()) { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.update(cx, |view, cx| {
        assert!(view.terminal.at_prompt());
        view.follow_history_scope(cx);
        view.input.editor_mut().set("echo recorded_during_pane_seed_184");
        view.submit_command(cx);
    });
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = view.update(cx, |view, cx| {
            view.follow_history_scope(cx);
            view.history.entries().iter().any(|line| line == "echo custom_histfile_seed_184")
        });
        if ready { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.update(cx, |view, cx| {
        view.handle_editor_key(&key("ctrl-r"), cx);
        view.commit_text("custom_histfile_seed_184", cx);
        assert_eq!(
            view.history.search().and_then(|search| search.selected_line(view.history.entries())),
            Some("echo custom_histfile_seed_184"),
            "reverse search must consume this pane's existing custom-HISTFILE seed",
        );
        assert!(!view.history.entries().iter().any(|line| line.contains("other_panes_private_seed_184")));
        view.history.cancel_search();
        view.handle_editor_key(&key("ctrl-r"), cx);
        view.commit_text("recorded_during_pane_seed_184", cx);
        assert_eq!(
            view.history.search().and_then(|search| search.selected_line(view.history.entries())),
            Some("echo recorded_during_pane_seed_184"),
            "loading the private seed must preserve commands submitted while it was reading",
        );
        view.history.cancel_search();
        view.handle_editor_key(&key("ctrl-r"), cx);
        view.commit_text("读取令牌", cx);
        assert_eq!(
            view.history.search().and_then(|search| search.selected_line(view.history.entries())),
            Some("echo '读取令牌失败ー🙂'"),
            "a pane-ID filename must not discard the actual zsh seed format",
        );
    });
    assert_eq!(std::fs::read(&custom).unwrap(), seed);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(other).unwrap();
}
