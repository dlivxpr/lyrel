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

fn pi_replay_assert_card_geometry(view: &TerminalView, expected: [[u8; 4]; 5], index: usize) {
    let image = native_picture_pixels(view, expected).unwrap();
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
        native_picture_wait(&view, cx, "saved tail picture", |view| {
            native_picture_pixels(view, NATIVE_PICTURE_LAST).is_some()
                && view.scrollback_text().contains("PI_REPLAY_TAIL")
        });
        graphics_assert_production_paint(window, &view, cx);
        let tail = view.read_with(cx, |view, _| {
            let image = native_picture_pixels(view, NATIVE_PICTURE_LAST).unwrap();
            assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
            assert!(image.anchor_col < 80 && image.placement.cols > 0);
            pi_replay_assert_card_geometry(view, NATIVE_PICTURE_LAST, 1);
            image.placement.image_id
        });
        // F5/F6 are public keybindings in the fixture's own agent directory.
        view.read_with(cx, |view, _| view.terminal.write(b"\x1b[15~".to_vec()));
        native_picture_wait(&view, cx, "native history remount", |view| {
            native_picture_pixels(view, NATIVE_PICTURE_FIRST).is_some()
                && native_picture_pixels(view, NATIVE_PICTURE_LAST).is_none()
        });
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            let first = native_picture_pixels(view, NATIVE_PICTURE_FIRST).unwrap();
            assert_ne!(first.placement.image_id, tail);
            assert!(first.painted.load(std::sync::atomic::Ordering::Acquire));
            pi_replay_assert_card_geometry(view, NATIVE_PICTURE_FIRST, 0);
        });
        view.read_with(cx, |view, _| view.terminal.write(b"\x1b[17~".to_vec()));
        native_picture_wait(&view, cx, "return to native tail", |view| {
            native_picture_pixels(view, NATIVE_PICTURE_LAST).is_some()
                && native_picture_pixels(view, NATIVE_PICTURE_FIRST).is_none()
        });
        // /new is a native UI command, not a prompt. It deliberately deletes the
        // old transcript. No terminal-side cache may revive those placements.
        view.read_with(cx, |view, _| view.terminal.write(b"/new\r".to_vec()));
        native_picture_wait(&view, cx, "native transcript deletion", |view| {
            graphics_visible_images(view).is_empty()
                && !view.scrollback_text().contains("PI_REPLAY_TAIL")
        });
        graphics_assert_production_paint(window, &view, cx);
        // Public app.exit on the empty editor. The next native process opens the
        // same saved source, and must retransmit; this is not daemon recovery.
        view.read_with(cx, |view, _| view.terminal.write(vec![4]));
        if round == 2 {
            native_picture_wait(&view, cx, "fixture cleanup and zero new messages", |view| {
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
    native_picture_wait(&view, cx, "scoped automatic Kitty discovery", |view| {
        native_picture_pixels(view, NATIVE_PICTURE_LAST).is_some()
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        pi_replay_assert_card_geometry(view, NATIVE_PICTURE_LAST, 1);
        let image = native_picture_pixels(view, NATIVE_PICTURE_LAST).unwrap();
        assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(graphics_visible_images(view).len(), 1);
    });
    view.read_with(cx, |view, _| view.terminal.write(vec![4]));
    native_picture_wait(&view, cx, "scoped replay audit and parent restoration", |view| {
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
        native_picture_wait(&view, cx, "native policy loaded saved transcript", |view| {
            view.scrollback_text().contains("PI_REPLAY_TAIL")
                && (policy != "settings-kitty" || native_picture_pixels(view, NATIVE_PICTURE_LAST).is_some())
        });
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            if policy == "settings-kitty" {
                assert!(
                    native_picture_pixels(view, NATIVE_PICTURE_LAST).is_some(),
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
        native_picture_wait(&view, cx, "disabled fixture clean exit", |view| {
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
