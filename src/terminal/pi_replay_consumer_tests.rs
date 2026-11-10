// Exercise the same production command encoder as launch/resume; no fixture
// writes terminal identity or forces the agent's image policy itself.
fn native_graphics_scoped_shell(agent: crate::core::cli_agent::CLIAgent, mut shell: crate::daemon::protocol::ShellSpec) -> crate::daemon::protocol::ShellSpec {
    let program = if cfg!(windows) { "pwsh" } else { "sh" };
    let line = std::iter::once(shell.program.clone()).chain(shell.args.iter().cloned())
        .map(|arg| lyrel_core::core::shell_quote::quote_for_shell(&arg, Some(program)))
        .collect::<Vec<_>>().join(" ");
    let line = lyrel_core::host::session_operations::agent_compatibility_command(agent, &line);
    let line = lyrel_core::host::session_operations::scoped_launch_command(
        &*lyrel_core::host::local::LocalHost::new(), &line, Some(program)).unwrap();
    let line = if cfg!(windows) {
        format!("Write-Output ('LYREL_PARENT_BEFORE=' + $env:TERM_PROGRAM); {line}; Write-Output ('LYREL_PARENT_AFTER=' + $env:TERM_PROGRAM)")
    } else {
        format!("printf 'LYREL_PARENT_BEFORE=%s\\n' \"$TERM_PROGRAM\"; {line}; printf 'LYREL_PARENT_AFTER=%s\\n' \"$TERM_PROGRAM\"")
    };
    shell.program = program.into();
    shell.args = if cfg!(windows) { vec!["-NoProfile".into(), "-Command".into(), line] } else { vec!["-c".into(), line] };
    shell
}

// Released Pi public-entry coverage, not a replacement conversation renderer.
// The fixture authors only synthetic saved JSONL; Pi emits every Kitty command.
struct PiReplayPane {
    daemon: GraphicsDaemonProcess,
    pane: u64,
}

impl PiReplayPane {
    fn start(policy: &str, rounds: u8) -> Self {
        let cli = std::env::var("LYREL_PI_GRAPHICS_CLI")
            .expect("set LYREL_PI_GRAPHICS_CLI to official Pi 1.4.0 executable or npm dist/cli.js");
        let node = std::env::var("LYREL_PI_GRAPHICS_NODE").unwrap_or_else(|_| "node".into());
        let daemon = GraphicsDaemonProcess::start();
        let shell = crate::daemon::protocol::ShellSpec {
            program: node,
            args: vec![
                concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/pi-image-replay.mjs").into(),
                "--pi".into(),
                cli,
                "--policy".into(),
                policy.into(),
                "--launch".into(),
                if policy == "enabled" { "legacy".into() } else { "scoped".into() },
                "--pictures".into(),
                "2".into(),
                "--rounds".into(),
                rounds.to_string(),
                "--temp-parent".into(),
                daemon.dir.path().to_string_lossy().into_owned(),
            ],
            args_are_lyrel_defaults: false,
        };
        let shell = if policy == "enabled" { shell } else {
            native_graphics_scoped_shell(crate::core::cli_agent::CLIAgent::Pi, shell)
        };
        let session = daemon
            .panes()
            .spawn(
                None,
                crate::daemon::protocol::WinSize {
                    cols: 80,
                    rows: 24,
                    cell_w: 10,
                    cell_h: 20,
                },
                Some(shell),
                None,
                None,
            )
            .unwrap();
        let pane = session.pane_id();
        session.detach().unwrap();
        Self { daemon, pane }
    }

    fn mount(&self, cx: &mut TestAppContext) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
        // Unlike the deterministic sender, the native TUI accepts no SEND commands.
        let (view, window) = graphics_build_rooted_consumer(
            cx,
            self.pane,
            self.daemon.stream(self.pane),
            Vec::new(),
        );
        graphics_assert_production_paint(window, &view, cx);
        (view, window)
    }
}

impl Drop for PiReplayPane {
    fn drop(&mut self) {
        let _ = self.daemon.panes().kill(self.pane);
    }
}

#[track_caller]
fn pi_replay_wait(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    description: &str,
    ready: impl Fn(&TerminalView) -> bool,
) {
    graphics_recovery_wait_with(
        cx,
        std::time::Duration::from_secs(45),
        |cx| view.read_with(cx, |view, _| ready(view)),
        |cx| view.read_with(cx, |view, _| format!(
            "Pi public-entry phase {description} timed out: child_exited={} disconnected={} text={:?} graphics={}",
            view.terminal.child_exited(),
            view.terminal.exited,
            view.scrollback_text(),
            view.terminal.images().recovery_diagnostics()
        )),
    );
}

fn pi_replay_pixels(
    view: &TerminalView,
    expected: [[u8; 4]; 5],
) -> Option<crate::terminal::images::RenderPlacement> {
    graphics_visible_images(view).into_iter().find(|image| {
        if image.evicted {
            return false;
        }
        let Some(data) = &image.data else {
            return false;
        };
        let Some(bytes) = data.as_bytes(0) else {
            return false;
        };
        if bytes.len() != 48 * 24 * 4 {
            return false;
        }
        // Compare the complete raster, not just dominant color or landmarks.
        bytes.chunks_exact(4).enumerate().all(|(index, pixel)| {
            let (x, y) = (index % 48, index / 48);
            let quadrant = if (22..26).contains(&x) && (10..14).contains(&y) { 4 }
                else if y < 12 { usize::from(x >= 24) } else { 2 + usize::from(x >= 24) };
            pixel == expected[quadrant]
        })
    })
}

fn pi_replay_assert_card_geometry(view: &TerminalView, expected: [[u8; 4]; 5], index: usize) {
    let image = pi_replay_pixels(view, expected).unwrap();
    let term = view.terminal.term.lock();
    let offset = term.display_offset() as i32;
    let row_of = |needle: &str| {
        (0..term.screen_lines())
            .find_map(|row| {
                let text: String = term.grid[Line(row as i32 - offset)][..]
                    .iter()
                    .map(|cell| cell.c())
                    .collect();
                text.contains(needle).then_some(row as i64)
            })
            .unwrap_or_else(|| panic!("missing native card witness {needle:?}"))
    };
    let call = row_of(&format!("synthetic-picture-{index}.png"));
    let after = row_of(if index == 0 {
        "Synthetic separator 0:0"
    } else {
        "PI_REPLAY_TAIL"
    });
    assert!(
        image.anchor_row > call && image.anchor_row + i64::from(image.placement.span_rows) <= after,
        "native image must be below its read card and above the following transcript, not in the editor; image={:?}, call={call}, after={after}",
        image.placement
    );
}

const PI_REPLAY_FIRST: [[u8; 4]; 5] = [
    [0, 0, 255, 255],
    [0, 255, 0, 255],
    [255, 0, 0, 255],
    [0, 255, 255, 255],
    [255, 255, 255, 255],
];
const PI_REPLAY_LAST: [[u8; 4]; 5] = [
    [11, 0, 255, 255],
    [7, 255, 0, 255],
    [255, 0, 5, 255],
    [0, 255, 255, 255],
    [255, 255, 255, 255],
];

#[gpui_kit::test]
#[ignore = "requires released Pi 1.4.0 and Node in LYREL_PI_GRAPHICS_CLI / LYREL_PI_GRAPHICS_NODE; no model calls"]
fn graphics_consumer_pi_native_saved_tool_images_remount_delete_and_resume(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let fixture = PiReplayPane::start("enabled", 2);
    let (view, window) = fixture.mount(cx);
    for round in 1..=2 {
        pi_replay_wait(&view, cx, "saved tail picture", |view| {
            pi_replay_pixels(view, PI_REPLAY_LAST).is_some()
                && view.scrollback_text().contains("PI_REPLAY_TAIL")
        });
        graphics_assert_production_paint(window, &view, cx);
        let tail = view.read_with(cx, |view, _| {
            let image = pi_replay_pixels(view, PI_REPLAY_LAST).unwrap();
            assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
            assert!(image.anchor_col < 80 && image.placement.cols > 0);
            pi_replay_assert_card_geometry(view, PI_REPLAY_LAST, 1);
            image.placement.image_id
        });
        // F5/F6 are public keybindings in the fixture's own agent directory.
        view.read_with(cx, |view, _| view.terminal.write(b"\x1b[15~".to_vec()));
        pi_replay_wait(&view, cx, "native history remount", |view| {
            pi_replay_pixels(view, PI_REPLAY_FIRST).is_some()
                && pi_replay_pixels(view, PI_REPLAY_LAST).is_none()
        });
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            let first = pi_replay_pixels(view, PI_REPLAY_FIRST).unwrap();
            assert_ne!(first.placement.image_id, tail);
            assert!(first.painted.load(std::sync::atomic::Ordering::Acquire));
            pi_replay_assert_card_geometry(view, PI_REPLAY_FIRST, 0);
        });
        view.read_with(cx, |view, _| view.terminal.write(b"\x1b[17~".to_vec()));
        pi_replay_wait(&view, cx, "return to native tail", |view| {
            pi_replay_pixels(view, PI_REPLAY_LAST).is_some()
                && pi_replay_pixels(view, PI_REPLAY_FIRST).is_none()
        });
        // /new is a native UI command, not a prompt. It deliberately deletes the
        // old transcript. No terminal-side cache may revive those placements.
        view.read_with(cx, |view, _| view.terminal.write(b"/new\r".to_vec()));
        pi_replay_wait(&view, cx, "native transcript deletion", |view| {
            graphics_visible_images(view).is_empty()
                && !view.scrollback_text().contains("PI_REPLAY_TAIL")
        });
        graphics_assert_production_paint(window, &view, cx);
        // Public app.exit on the empty editor. The next native process opens the
        // same saved source, and must retransmit; this is not daemon recovery.
        view.read_with(cx, |view, _| view.terminal.write(vec![4]));
        if round == 2 {
            pi_replay_wait(&view, cx, "fixture cleanup and zero new messages", |view| {
                view.scrollback_text().contains("PI_REPLAY_FIXTURE_DONE")
                    && view
                        .scrollback_text()
                        .contains("PI_REPLAY_ROUND_2_NO_NEW_MESSAGES")
            });
        }
    }
}

#[gpui_kit::test]
#[ignore = "released Pi 1.4.0 + Node; production scoped discovery without a forced image protocol"]
fn graphics_consumer_pi_native_scoped_discovery_replays_without_forcing_protocol(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let fixture = PiReplayPane::start("scoped", 1);
    let (view, window) = fixture.mount(cx);
    pi_replay_wait(&view, cx, "scoped automatic Kitty discovery", |view| {
        pi_replay_pixels(view, PI_REPLAY_LAST).is_some()
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        pi_replay_assert_card_geometry(view, PI_REPLAY_LAST, 1);
        let image = pi_replay_pixels(view, PI_REPLAY_LAST).unwrap();
        assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(graphics_visible_images(view).len(), 1);
    });
    view.read_with(cx, |view, _| view.terminal.write(vec![4]));
    pi_replay_wait(&view, cx, "scoped replay audit and parent restoration", |view| {
        let text = view.scrollback_text();
        text.contains("PI_REPLAY_FIXTURE_DONE") && text.contains("LYREL_PARENT_AFTER=lyrel")
            && text.contains("PI_REPLAY_ROUND_1_NO_NEW_MESSAGES")
    });
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
#[ignore = "requires released Pi 1.4.0 and Node; explicit native disabled/setting precedence, no model calls"]
fn graphics_consumer_pi_native_explicit_image_policies_take_precedence(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    for policy in [
        "env-none",
        "settings-none",
        "display-none",
        "settings-kitty",
    ] {
        let fixture = PiReplayPane::start(policy, 1);
        let (view, window) = fixture.mount(cx);
        pi_replay_wait(&view, cx, "native policy loaded saved transcript", |view| {
            view.scrollback_text().contains("PI_REPLAY_TAIL")
                && (policy != "settings-kitty" || pi_replay_pixels(view, PI_REPLAY_LAST).is_some())
        });
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            if policy == "settings-kitty" {
                assert!(
                    pi_replay_pixels(view, PI_REPLAY_LAST).is_some(),
                    "settings override PI_IMAGE_PROTOCOL=none"
                );
            } else {
                assert!(
                    graphics_visible_images(view).is_empty(),
                    "{policy} must not emit native pictures"
                );
            }
        });
        view.read_with(cx, |view, _| view.terminal.write(vec![4]));
        pi_replay_wait(&view, cx, "disabled fixture clean exit", |view| {
            view.scrollback_text().contains("PI_REPLAY_FIXTURE_DONE")
                && view
                    .scrollback_text()
                    .contains("PI_REPLAY_ROUND_1_NO_NEW_MESSAGES")
        });
        drop(view);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
}
