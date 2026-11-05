#[cfg(windows)]
fn keyboard_boundary_real_pty_case(cx: &mut TestAppContext, prior_mode: &str) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let bash = crate::core::shells::detect_shells()
        .into_iter()
        .find(|shell| shell.label == "Git Bash")
        .expect("the real ConPTY keyboard regression requires Git Bash");
    let daemon = GraphicsDaemonProcess::start_with(|command, dir| {
        let home = dir.join("home");
        std::fs::create_dir(&home).unwrap();
        let history = home.join("synthetic_history");
        std::fs::write(&history, b"echo isolated-keyboard-fixture\n").unwrap();
        command
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("HISTFILE", history)
            .env_remove("LYREL_SHELL_INTEGRATION")
            .env_remove("LYREL_HISTFILE")
            .env_remove("PROMPT_COMMAND")
            .env_remove("BASH_ENV")
            .env_remove("ENV");
    });
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/support/kitty_keyboard_boundary.mjs")
        .to_string_lossy()
        .replace('\\', "/");
    let size = crate::daemon::protocol::WinSize {
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    };
    let session = daemon
        .panes()
        .spawn(
            Some(daemon.dir.path().to_path_buf()),
            size,
            Some(crate::daemon::protocol::ShellSpec {
                program: bash.program,
                args: Vec::new(),
                args_are_lyrel_defaults: true,
            }),
            None,
            None,
        )
        .unwrap();
    let pane = session.pane_id();
    session.detach().unwrap();
    let mut observer = daemon.panes().observe(pane, size).unwrap();
    observer
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
        crate::ui::keymap::init(cx);
    });
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    graphics_recovery_wait(cx, "real production Bash integration ready", |cx| {
        view.read_with(cx, |view, _| view.terminal.at_prompt())
    });
    let cycle = view.read_with(cx, |view, _| view.terminal.prompt_cycle());
    // Nothing in this fixture manufactures shell/agent OSC marks. The real
    // default-shell wrapper owns C/D and A/B, after its real Node child exits.
    let command = format!("node '{}' {prior_mode}\n", fixture.replace('\'', "'\\''"));
    view.read_with(cx, |view, _| view.terminal.write(command.into_bytes()));
    graphics_recovery_wait(cx, "real shell returned after prior program", |cx| {
        view.read_with(cx, |view, _| {
            view.terminal.at_prompt() && view.terminal.prompt_cycle() > cycle
        })
    });
    let mut output = Vec::new();
    while !output
        .windows(b"133;D;0;aid=lyrel\x07".len())
        .any(|part| part == b"133;D;0;aid=lyrel\x07")
    {
        match observer.recv().unwrap() {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => output.extend(bytes),
            DaemonMsg::Exited { code } => panic!("integrated Bash exited: {code:?}"),
            _ => {}
        }
    }
    eprintln!("production shell owned completion observed; prior={prior_mode}");
    let command = format!("node '{}' record\n", fixture.replace('\'', "'\\''"));
    view.read_with(cx, |view, _| view.terminal.write(command.into_bytes()));
    graphics_recovery_wait(cx, "next actual PTY program ready", |cx| {
        view.read_with(cx, |view, _| view.title == "KEYBOARD_RECORDER_READY")
    });
    window
        .update(cx, |_, window, cx| {
            window.activate_window();
            let focus = view.read(cx).focus_handle.clone();
            focus.focus(window, cx);
        })
        .unwrap();
    {
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.simulate_keystrokes("escape ctrl-c");
    }
    view.read_with(cx, |view, _| view.terminal.write(b"\n".to_vec()));
    graphics_recovery_wait(cx, "PTY reported actual keystroke bytes", |cx| {
        view.read_with(cx, |view, _| view.title.starts_with("KEYBOARD_BYTES:"))
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.title.clone()),
        "KEYBOARD_BYTES:1b03",
        "Escape and Ctrl+C must reach the next real program as legacy bytes, not dead Kitty modes"
    );
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[cfg(windows)]
#[gpui_kit::test]
fn keyboard_boundary_real_pty_clean_program_preserves_keys(cx: &mut TestAppContext) {
    keyboard_boundary_real_pty_case(cx, "clean");
}

#[cfg(windows)]
#[gpui_kit::test]
fn keyboard_boundary_real_pty_shell_return_preserves_escape_and_ctrl_c(cx: &mut TestAppContext) {
    keyboard_boundary_real_pty_case(cx, "dirty");
}

#[gpui_kit::test]
fn keyboard_boundary_replay_and_relink_keep_only_the_current_program_modes(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
        crate::ui::keymap::init(cx);
    });
    let scenarios: &[(&[u8], &[u8], &[u8])] = &[
        (
            b"\x1b[>7u\x1b[?1049h\x1b[>7u\x1b[?1049l\x1b]133;D;130;aid=lyrel\x07",
            b"\x1b[?1049h",
            b"\x1b",
        ),
        (
            b"\x1b[>7u\x1b]133;D;0;aid=lyrel\x1b\\",
            b"\x1b[>1u\x1b[<u",
            b"\x1b",
        ),
        (
            b"\x1b[>7u\x1b]133;D;0;aid=lyrel\x07",
            b"\x1b[>1u",
            b"\x1b[27u",
        ),
        (
            b"\x1b[>1u\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07",
            b"",
            b"\x1b[27u",
        ),
        (b"\x1b[>1u\x1b]133;D;0;aid=other\x07", b"", b"\x1b[27u"),
    ];
    for &(history, live, expected) in scenarios {
        for relink in [false, true] {
            for evicted in [false, true] {
                let mut modes = lyrel_core::core::term_modes::TerminalModes::new();
                for byte in history {
                    modes.feed(&[*byte]);
                }
                let prelude = modes.restore_bytes().unwrap_or_default();
                let replay = if evicted { prelude.as_slice() } else { history };
                let (client, mut sender) = test_stream_pair();
                let (view, window) = graphics_build_rooted_consumer_at(
                    cx,
                    188,
                    client,
                    Vec::new(),
                    TermSize::new(80, 24),
                    PtySource::Raw,
                );
                if relink {
                    let (client, fresh_sender) = test_stream_pair();
                    view.update(cx, |view, cx| {
                        view.adopt_relink(
                            client,
                            Vec::new(),
                            &crate::terminal::PaneRoute::Local,
                            TermSize::new(80, 24),
                            10,
                            20,
                            cx,
                        )
                        .unwrap();
                    });
                    sender = fresh_sender;
                }
                keyboard_boundary_replay_keys(
                    cx,
                    view,
                    window,
                    &mut sender,
                    replay,
                    live,
                    expected,
                );
            }
        }
    }
}

fn keyboard_boundary_replay_keys(
    cx: &mut TestAppContext,
    view: Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    sender: &mut Stream,
    history: &[u8],
    live: &[u8],
    expected: &[u8],
) {
    // Fragment every byte through the production reader, including OSC ST.
    for byte in history {
        DaemonMsg::Snapshot(vec![*byte]).encode(sender).unwrap();
    }
    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: Some(0),
    }
    .encode(sender)
    .unwrap();
    DaemonMsg::Output([live, b"\x1b]2;KEYBOARD_REPLAY_READY\x07"].concat())
        .encode(sender)
        .unwrap();
    graphics_recovery_wait(cx, "fragmented keyboard replay", |cx| {
        view.read_with(cx, |view, _| view.title == "KEYBOARD_REPLAY_READY")
    });
    window
        .update(cx, |_, window, cx| {
            window.activate_window();
            let focus = view.read(cx).focus_handle.clone();
            focus.focus(window, cx);
        })
        .unwrap();
    {
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.simulate_keystrokes("escape");
    }
    assert_eq!(
        next_input_until_timeout(sender).as_deref(),
        Some(expected),
        "history={history:?}, live={live:?}"
    );
    graphics_recovery_destroy(view, window, cx);
}
