// This file is included beside the existing production graphics consumer harness.
struct OmpReplayFixture {
    root: tempfile::TempDir,
    streaming: bool,
    scoped: bool,
}

impl OmpReplayFixture {
    fn prepare(case: &str, mode: &str) -> Self {
        let omp = std::env::var_os("LYREL_TEST_OMP")
            .expect("set LYREL_TEST_OMP to an installed released omp executable");
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("node")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/omp-native-replay.mjs"
            ))
            .args(["prepare", "--root"])
            .arg(root.path().join("owned"))
            .arg("--omp")
            .arg(omp)
            .args(["--case", case, "--mode", mode])
            .status()
            .unwrap();
        assert!(status.success(), "preparing public synthetic omp history");
        Self {
            root,
            streaming: case == "streaming",
            scoped: matches!(case, "streaming" | "disabled" | "protocol-off" | "placeholder-off"),
        }
    }

    fn spawn(&self, daemon: &GraphicsDaemonProcess) -> u64 {
        let mut shell = crate::daemon::protocol::ShellSpec {
            program: "node".into(),
            args: vec![
                concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/omp-native-replay.mjs").into(),
                "run".into(),
                "--wait-ready".into(),
                "--root".into(),
                self.root
                    .path()
                    .join("owned")
                    .to_string_lossy()
                    .into_owned(),
            ],
            args_are_lyrel_defaults: false,
        };
        if self.scoped {
            let program = if cfg!(windows) { "pwsh" } else { "sh" };
            let line = std::iter::once(shell.program.clone())
                .chain(shell.args.iter().cloned())
                .map(|arg| lyrel_core::core::shell_quote::quote_for_shell(&arg, Some(program)))
                .collect::<Vec<_>>()
                .join(" ");
            let scoped = lyrel_core::host::session_operations::agent_compatibility_command(
                crate::core::cli_agent::CLIAgent::OhMyPi,
                &line,
            );
            let scoped = lyrel_core::host::session_operations::scoped_launch_command(
                &*lyrel_core::host::local::LocalHost::new(),
                &scoped,
                Some(program),
            )
            .unwrap();
            let line = if cfg!(windows) {
                format!("Write-Output ('LYREL_PARENT_BEFORE=' + $env:TERM_PROGRAM); {scoped}; Write-Output ('LYREL_PARENT_AFTER=' + $env:TERM_PROGRAM)")
            } else {
                format!("printf 'LYREL_PARENT_BEFORE=%s\\n' \"$TERM_PROGRAM\"; {scoped}; printf 'LYREL_PARENT_AFTER=%s\\n' \"$TERM_PROGRAM\"")
            };
            shell.program = program.into();
            shell.args = if cfg!(windows) {
                vec!["-NoProfile".into(), "-Command".into(), line]
            } else {
                vec!["-c".into(), line]
            };
        }
        let session = daemon
            .panes()
            .spawn(
                Some(self.root.path().to_path_buf()),
                crate::daemon::protocol::WinSize {
                    cols: if self.streaming { 100 } else { 80 },
                    rows: if self.streaming { 48 } else { 24 },
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
        pane
    }

    fn stage(&self) -> Option<String> {
        let bytes = std::fs::read(self.root.path().join("owned/stage.json")).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        value["stage"].as_str().map(str::to_owned)
    }

    fn diagnostics(&self, view: &Entity<TerminalView>, cx: &mut TestAppContext) -> String {
        let terminal = view.update(cx, |view, _| {
            view.terminal.poll_exited();
            format!(
                "pane={} child_exited={} disconnected={} text={:?} graphics={}",
                view.pane_id,
                view.terminal.child_exited(),
                view.terminal.exited,
                view.scrollback_text(),
                view.terminal.images().recovery_diagnostics()
            )
        });
        let mut result = format!("owned_root={} {terminal}", self.root.path().display());
        for name in ["fixture.json", "stages.jsonl", "version.json", "identity.json", "config-path.json", "config-terminal.showImages.json", "config-tui.renderSvg.json", "config-tui.maxInlineImages.json", "models.json", "running.json", "requests.json", "audit.json"] {
            if let Ok(text) = std::fs::read_to_string(self.root.path().join("owned").join(name)) {
                result.push_str(&format!("\n{name}: {text}"));
            }
        }
        result
    }

    #[track_caller]
    fn assert_running(&self, view: &Entity<TerminalView>, cx: &mut TestAppContext, phase: &str) {
        let failed = self.stage().as_deref() == Some("failed");
        let exited = view.read_with(cx, |view, _| view.terminal.child_exited());
        if failed || exited {
            panic!("omp public-entry phase {phase} failed: {}", self.diagnostics(view, cx));
        }
    }

    fn start(&self, view: &Entity<TerminalView>, cx: &mut TestAppContext) {
        graphics_recovery_wait_with(
            cx,
            std::time::Duration::from_secs(30),
            |cx| {
                if self.stage().as_deref() == Some("waiting-consumer") { return true; }
                self.assert_running(view, cx, "fixture readiness before native launch");
                false
            },
            |cx| format!("omp fixture readiness timed out: {}", self.diagnostics(view, cx)),
        );
        // Readiness is not a prompt and must never share omp's ConPTY stdin.
        let active: serde_json::Value = serde_json::from_slice(
            &std::fs::read(self.root.path().join("owned/running.json")).unwrap()
        ).unwrap();
        assert!(active["childPid"].is_null(), "native child started before rooted readiness: {active}");
        std::fs::write(self.root.path().join("owned/consumer-ready"), b"rooted production consumer mounted\n").unwrap();
    }

    fn wait_replay(&self, view: &Entity<TerminalView>, cx: &mut TestAppContext) {
        // Do not scroll into stable reading while omp is still constructing
        // its saved transcript. The tail is emitted by the real native TUI;
        // decode/identity/placement and fresh paint checks still follow it.
        graphics_recovery_wait_with(
            cx,
            std::time::Duration::from_secs(90),
            |cx| {
                self.assert_running(view, cx, "saved transcript tail");
                view.read_with(cx, |view, _| {
                    view.scrollback_text().contains("LYREL_REPLAY_END")
                        && !view.terminal.term.lock().synchronized_output()
                })
            },
            |cx| format!("omp transcript timed out: {}", self.diagnostics(view, cx)),
        );
    }

    fn finish(&self, view: &Entity<TerminalView>, cx: &mut TestAppContext) {
        if self.streaming {
            // The public JSONL must contain the completed turn before /exit;
            // receiving the SSE terminator alone is not agent completion.
            graphics_recovery_wait_with(
                cx,
                std::time::Duration::from_secs(30),
                |cx| {
                    self.assert_running(view, cx, "native streaming completion");
                    std::fs::read_to_string(self.root.path().join("owned/workspace/pictures.jsonl"))
                        .ok()
                        .is_some_and(|text| text.lines().any(|line| {
                            serde_json::from_str::<serde_json::Value>(line).is_ok_and(|record| {
                                record["type"] == "message"
                                    && record["message"]["model"] == "stream-svg"
                                    && record["message"]["stopReason"] == "stop"
                            })
                        }))
                },
                |cx| self.diagnostics(view, cx),
            );
        }
        view.read_with(cx, |view, _| view.terminal.write(b"/exit\r".to_vec()));
        let audit = self.root.path().join("owned/audit.json");
        graphics_recovery_wait_with(
            cx,
            std::time::Duration::from_secs(30),
            |cx| {
                if self.stage().as_deref() == Some("failed") {
                    panic!("omp public-entry phase native exit/audit failed: {}", self.diagnostics(view, cx));
                }
                audit.exists()
            },
            |cx| format!("omp native exit/audit timed out: {}", self.diagnostics(view, cx)),
        );
        let audit: serde_json::Value =
            serde_json::from_slice(&std::fs::read(audit).unwrap()).unwrap();
        if self.streaming {
            assert_eq!(audit["loopbackStreaming"], true, "{audit}");
            assert_eq!(audit["zeroInference"], false, "streaming is NOT static replay: {audit}");
            graphics_recovery_wait_with(
                cx,
                std::time::Duration::from_secs(10),
                |cx| view.read_with(cx, |view, _| view.scrollback_text().contains("LYREL_PARENT_AFTER=lyrel")),
                |cx| self.diagnostics(view, cx),
            );
            assert!(view.read_with(cx, |view, _| view.scrollback_text().contains("LYREL_PARENT_BEFORE=lyrel")));
        } else {
            assert_eq!(audit["zeroInference"], true, "{audit}");
        }
        if self.scoped {
            let identity: serde_json::Value = serde_json::from_slice(
                &std::fs::read(self.root.path().join("owned/identity.json")).unwrap(),
            ).unwrap();
            assert_eq!(identity["TERM_PROGRAM"], "kitty", "opt-outs must win even when native auto-detection recognizes Kitty");
            assert_eq!(identity["TERM_PROGRAM_VERSION"], "");
            assert_eq!(identity["PI_NO_DECCARA"], "1");
            assert_eq!(identity["LC_TERMINAL"], "lyrel");
            assert_eq!(identity["LC_TERMINAL_VERSION"], identity["LYREL"]);
        }
        assert_eq!(audit["exitCode"], 0, "{audit}");
        eprintln!("released omp replay evidence: {audit}");
    }
}

impl Drop for OmpReplayFixture {
    fn drop(&mut self) {
        // A failed assertion must not strand this fixture's native TUI child.
        let active = self.root.path().join("owned/running.json");
        let Ok(bytes) = std::fs::read(active) else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return;
        };
        let Some(pid) = value["pid"].as_u64() else {
            return;
        };
        if cfg!(windows) {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status();
        } else {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct OmpReplayPictureIdentity {
    image_id: u32,
    placement_id: u32,
    anchor_row: i64,
    anchor_col: i64,
}

impl OmpReplayPictureIdentity {
    fn from_image(image: &crate::terminal::images::RenderPlacement) -> Self {
        Self {
            image_id: image.placement.image_id,
            placement_id: image.placement.placement_id,
            anchor_row: image.anchor_row,
            anchor_col: image.anchor_col,
        }
    }
}

fn omp_replay_scan(
    view: &Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    cx: &mut TestAppContext,
) -> std::collections::BTreeMap<Vec<u8>, OmpReplayPictureIdentity> {
    let mut found = std::collections::BTreeMap::new();
    let history = view.read_with(cx, |view, _| view.terminal.term.lock().grid.history_size());
    view.update(cx, |view, cx| {
        view.scroll(-100_000, None, &Modifiers::default(), cx)
    });
    for _ in 0..=(history / 12 + 2) {
        graphics_assert_production_paint(window, view, cx);
        view.read_with(cx, |view, _| {
            for image in graphics_visible_images(view) {
                let Some(data) = &image.data else {
                    continue;
                };
                let bytes = data.as_bytes(0).unwrap();
                if bytes.len() == 64 * 32 * 4 {
                    assert_eq!(&bytes[(16 * 64 + 32) * 4..][..4], &[0, 0, 0, 255]);
                    found.insert(
                        bytes[..4].to_vec(),
                        OmpReplayPictureIdentity::from_image(&image),
                    );
                } else if bytes
                    .chunks_exact(4)
                    .any(|pixel| pixel == [86, 52, 18, 255])
                    && bytes
                        .chunks_exact(4)
                        .any(|pixel| pixel == [0, 128, 255, 255])
                {
                    found.insert(
                        b"rasterized-svg".to_vec(),
                        OmpReplayPictureIdentity::from_image(&image),
                    );
                }
                if image.painted.load(std::sync::atomic::Ordering::Acquire) {
                    let (crop, _) =
                        image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), history, 0, 1.);
                    assert!(crop.size.width > px(0.) && crop.size.height > px(0.));
                }
            }
        });
        view.update(cx, |view, cx| {
            view.scroll(12, None, &Modifiers::default(), cx)
        });
    }
    found
}

fn omp_replay_assert_resume(cx: &mut TestAppContext, cases: &[&str]) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    for &case in cases {
        let fixture = OmpReplayFixture::prepare(case, "placeholders");
        let daemon = GraphicsDaemonProcess::start();
        let pane = fixture.spawn(&daemon);
        let (view, window) =
            graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
        graphics_assert_production_paint(window, &view, cx);
        fixture.start(&view, cx);
        fixture.wait_replay(&view, cx);
        let expected = if case == "budget" { 8 } else { 3 };
        let mut found = std::collections::BTreeMap::new();
        graphics_recovery_wait_with(
            cx,
            std::time::Duration::from_secs(90),
            |cx| {
                found = omp_replay_scan(&view, window, cx);
                if found.len() == expected && found.contains_key(b"rasterized-svg".as_slice()) { return true; }
                fixture.assert_running(&view, cx, &format!("{case} decoded identities: {found:?}"));
                false
            },
            |cx| format!("omp {case} identities timed out: {}", fixture.diagnostics(&view, cx)),
        );
        assert_eq!(
            found
                .values()
                .map(|identity| identity.image_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            expected,
            "each expected native picture must own a distinct image id: {found:?}"
        );
        assert!(
            found.contains_key([192, 128, 240, 255].as_slice()),
            "native tool-result pixels"
        );
        if case != "budget" {
            let assistant = found.get([0, 0, 255, 255].as_slice()).unwrap();
            let tool = found.get([192, 128, 240, 255].as_slice()).unwrap();
            let figure = found.get(b"rasterized-svg".as_slice()).unwrap();
            assert_ne!(assistant.image_id, tool.image_id);
            assert_ne!(tool.image_id, figure.image_id);
            assert!(
                assistant.anchor_row < tool.anchor_row && tool.anchor_row < figure.anchor_row,
                "ordered native image placements: {found:?}"
            );
            if case == "placeholder-off" {
                view.read_with(cx, |view, _| {
                    let direct: std::collections::BTreeSet<_> = view.terminal.images().snapshot()
                        .iter().map(|image| image.placement.image_id).collect();
                    let expected: std::collections::BTreeSet<_> = found.values().map(|image| image.image_id).collect();
                    assert_eq!(direct, expected, "placeholder opt-out uses native direct placements for all three rasters");
                    assert!(!view.scrollback_text().contains('\u{10eeee}'), "no native Unicode placeholder cells when explicitly disabled");
                });
            }
        } else {
            for color in [
                [0, 0, 255, 255],
                [0, 255, 0, 255],
                [255, 0, 0, 255],
                [0, 255, 255, 255],
            ] {
                assert!(
                    !found.contains_key(color.as_slice()),
                    "upstream d=I demotion must not revive old image"
                );
            }
            for color in [
                [255, 0, 255, 255],
                [255, 255, 0, 255],
                [0, 96, 192, 255],
                [192, 0, 96, 255],
                [96, 128, 0, 255],
                [128, 128, 128, 255],
            ] {
                assert!(
                    found.contains_key(color.as_slice()),
                    "wrong recent native image identity: {found:?}"
                );
            }
            assert_eq!(
                omp_replay_scan(&view, window, cx),
                found,
                "scroll-out/back must preserve deletion and exact placement identity"
            );
        }
        fixture.finish(&view, cx);
        graphics_recovery_destroy(view, window, cx);
    }
}

#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP, Node and a real released omp; never calls a model"]
fn graphics_consumer_released_omp_native_resume_pictures(cx: &mut TestAppContext) {
    omp_replay_assert_resume(cx, &["pictures"]);
}

#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP, Node and a real released omp; never calls a model"]
fn graphics_consumer_released_omp_native_resume_budget(cx: &mut TestAppContext) {
    omp_replay_assert_resume(cx, &["budget"]);
}

#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP; real native disabled-policy resume"]
fn graphics_consumer_released_omp_native_disabled_policy(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    for case in ["disabled", "protocol-off"] {
        let fixture = OmpReplayFixture::prepare(case, "placeholders");
        let daemon = GraphicsDaemonProcess::start();
        let pane = fixture.spawn(&daemon);
        let (view, window) =
            graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
        graphics_assert_production_paint(window, &view, cx);
        fixture.start(&view, cx);
        fixture.wait_replay(&view, cx);
        assert!(
            omp_replay_scan(&view, window, cx).is_empty(),
            "explicit disabled is authoritative"
        );
        fixture.finish(&view, cx);
        graphics_recovery_destroy(view, window, cx);
    }
}

#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP, Node and released omp; native direct-placement opt-out"]
fn graphics_consumer_released_omp_native_placeholder_opt_out(cx: &mut TestAppContext) {
    omp_replay_assert_resume(cx, &["placeholder-off"]);
}

// This is token streaming through released omp, not the zero-request replay above.
// Provider gates advance only after the rooted production consumer has observed
// a complete raster; a slow machine cannot silently coalesce away a revision.
#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP=release 18.18.6, Node and real PTY; loopback SSE only"]
fn graphics_consumer_released_omp_loopback_streamed_svg(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let fixture = OmpReplayFixture::prepare("streaming", "placeholders");
    let daemon = GraphicsDaemonProcess::start();
    let pane = fixture.spawn(&daemon);
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(100, 48));
    fixture.start(&view, cx);
    let wait_text = |cx: &mut TestAppContext, needle: &str| {
        graphics_recovery_wait_with(
            cx,
            std::time::Duration::from_secs(90),
            |cx| {
                fixture.assert_running(&view, cx, needle);
                view.read_with(cx, |view, _| view.scrollback_text().contains(needle))
            },
            |cx| format!("missing native text {needle}: {}", fixture.diagnostics(&view, cx)),
        );
    };
    wait_text(cx, "LYREL_STREAM_READY");
    view.read_with(cx, |view, _| view.terminal.write(b"/debug\r".to_vec()));
    wait_text(cx, "Debug Tools");
    // The pinned public /debug menu's eighth entry is View: terminal state.
    view.read_with(cx, |view, _| view.terminal.write(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\r".to_vec()));
    wait_text(cx, "rectangular-SGR background fills");
    let text = view.read_with(cx, |view, _| view.scrollback_text());
    assert!(text.lines().any(|line| line.contains("Detected:") && line.contains("kitty")), "{text}");
    assert!(text.lines().any(|line| line.contains("Graphics:") && line.contains("Kitty graphics")), "{text}");
    assert!(text.lines().any(|line| line.contains("DECCARA:") && line.contains("no (rectangular")), "{text}");
    eprintln!("released omp native terminal diagnostics (production text):\n{text}");
    let cell = text.lines().find_map(|line| {
        let (_, suffix) = line.split_once("cells · cell ")?;
        let (width, height) = suffix.trim().strip_suffix("px")?.split_once('x')?;
        Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?))
    }).expect("released omp's visible native cell geometry");
    view.read_with(cx, |view, _| view.terminal.write(b"LYREL_LOOPBACK_PROMPT\r".to_vec()));
    let mut revisions = Vec::new();
    for revision in 1..=3 {
        let mut observed = None;
        graphics_recovery_wait_with(
            cx,
            std::time::Duration::from_secs(45),
            |cx| {
                fixture.assert_running(&view, cx, "live SVG revision");
                graphics_assert_production_paint_at(window, &view, cx, TermSize::new(100, 48));
                observed = view.read_with(cx, |view, _| {
                    if revision > 1 {
                        assert!(!graphics_visible_images(view).is_empty(), "never blank an established streaming figure between complete revisions");
                    }
                    omp_stream_raster(view, cell, revision)
                });
                observed.is_some()
            },
            |cx| format!("missing complete streamed revision {revision}: {}", fixture.diagnostics(&view, cx)),
        );
        let identity = observed.unwrap();
        eprintln!("released omp streaming production evidence: revision={revision} cell={cell:?} identity={identity:?}; complete exact BGRA, whole-texture crop, z=0, painted; not GPU pixels");
        assert!(revisions.iter().all(|old: &OmpReplayPictureIdentity| old.image_id != identity.image_id));
        revisions.push(identity);
        std::fs::write(fixture.root.path().join(format!("owned/observed-{revision}")), b"complete production paint\n").unwrap();
        if revision < 3 {
            graphics_recovery_wait_with(
                cx,
                std::time::Duration::from_secs(30),
                |_| fixture.stage().as_deref() == Some(format!("partial-{revision}").as_str()),
                |cx| fixture.diagnostics(&view, cx),
            );
            // Deliberately incomplete XML must retain the last complete raster.
            for _ in 0..3 {
                graphics_assert_production_paint_at(window, &view, cx, TermSize::new(100, 48));
                view.read_with(cx, |view, _| {
                    assert!(omp_stream_raster(view, cell, revision).is_some(), "incomplete XML keeps complete prior pixels, even if omp re-rasterizes the repaired source");
                });
            }
            std::fs::write(fixture.root.path().join(format!("owned/partial-observed-{revision}")), b"previous raster retained\n").unwrap();
        }
    }
    wait_text(cx, "LYREL_STREAM_AFTER");
    graphics_recovery_wait_with(
        cx,
        std::time::Duration::from_secs(30),
        |_| fixture.stage().as_deref() == Some("stream-complete"),
        |cx| fixture.diagnostics(&view, cx),
    );
    for _ in 0..3 {
        graphics_assert_production_paint_at(window, &view, cx, TermSize::new(100, 48));
        view.read_with(cx, |view, _| {
            assert_eq!(omp_stream_raster(view, cell, 3).as_ref(), revisions.last());
            let term = view.terminal.term.lock();
            let origin = term.grid.lines_evicted() as i64 + term.grid.history_size() as i64;
            let row_of = |needle: &str| {
                (-(term.grid.history_size() as i32)..term.screen_lines() as i32).find_map(|row| {
                    let text: String = term.grid[Line(row)][..].iter().map(|cell| cell.c()).collect();
                    text.contains(needle).then_some(origin + i64::from(row))
                }).unwrap_or_else(|| panic!("missing surrounding native text {needle}"))
            };
            let image = revisions.last().unwrap();
            assert!(row_of("LYREL_STREAM_BEFORE") < image.anchor_row);
            assert!(image.anchor_row + 2 <= row_of("LYREL_STREAM_AFTER"));
            let rows: Vec<_> = (-(term.grid.history_size() as i32)..term.screen_lines() as i32)
                .filter_map(|row| {
                    let slices = lyrel_core::core::kitty_graphics::placeholder_row(&term.grid, Line(row));
                    (!slices.is_empty()).then_some((row, slices))
                })
                .collect();
            drop(term);
            let live = view.terminal.images().snapshot_with_placeholders(&rows);
            assert!(!live.is_empty(), "final figure remains in production presentation");
            assert!(live.iter().all(|placement| placement.placement.image_id == image.image_id), "no superseded placements may survive: {:?}", live.iter().map(|image| &image.placement).collect::<Vec<_>>());
        });
    }
    fixture.finish(&view, cx);
    graphics_recovery_destroy(view, window, cx);
}

// The independent SVG scene is 64x32 units: red, then a green right half,
// then a blue bottom half. Compare EVERY decoded BGRA pixel, including native
// whole-cell transparent right padding; this is not a hash learned at runtime.
fn omp_stream_raster(
    view: &TerminalView,
    (cell_width, cell_height): (u32, u32),
    revision: u32,
) -> Option<OmpReplayPictureIdentity> {
    let drawing_width = 4 * cell_height;
    let width = drawing_width.div_ceil(cell_width) * cell_width;
    let height = 2 * cell_height;
    let images = graphics_visible_images(view);
    if images.is_empty() {
        return None;
    }
    let mut identity: Option<OmpReplayPictureIdentity> = None;
    let geom = view.cell_geom.expect("rooted production paint geometry");
    let offset = view.terminal.term.lock().display_offset() as i32;
    let mut texture = None;
    let mut area = 0.0_f32;
    for image in images {
        if image.evicted {
            return None;
        }
        let bytes = image.data.as_ref()?.as_bytes(0)?;
        assert_eq!(bytes.len(), (width * height * 4) as usize, "never expose partial PNG pixels");
        let actual_revision = if bytes[((height / 2 * width) * 4) as usize..][..4] == [255, 0, 0, 255] {
            3
        } else if bytes[((width.min(drawing_width) / 2) * 4) as usize..][..4] == [0, 255, 0, 255] {
            2
        } else {
            1
        };
        for y in 0..height {
            for x in 0..width {
                let expected = if x >= drawing_width {
                    [0, 0, 0, 0]
                } else if actual_revision == 3 && y >= cell_height {
                    [255, 0, 0, 255]
                } else if actual_revision >= 2 && x >= drawing_width / 2 {
                    [0, 255, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                assert_eq!(bytes[((y * width + x) * 4) as usize..][..4], expected, "complete raster {actual_revision}, pixel ({x},{y})");
            }
        }
        if actual_revision != revision {
            return None;
        }
        assert_eq!(image.placement.z, 0, "native above-text Kitty layer");
        assert_eq!((image.placement.x, image.placement.y, image.placement.w, image.placement.h), (0, 0, width, height));
        assert!(image.painted.load(std::sync::atomic::Ordering::Acquire), "actual production paint claim, not GPU pixels");
        let (crop, full) = image.paint_bounds(geom.origin, geom.cell_width, geom.line_height, 0, offset, 1.);
        assert!(crop.size.width > px(0.) && crop.size.height > px(0.));
        assert_eq!(crop.intersect(&full), crop, "native placeholder crop stays within its complete texture");
        if let Some(previous) = texture {
            assert_eq!(previous, full, "all native placeholder slices align to one texture");
        }
        texture = Some(full);
        area += crop.size.width.as_f32() * crop.size.height.as_f32();
        let current = OmpReplayPictureIdentity::from_image(&image);
        if let Some(previous) = &identity {
            // Placeholder slices share the same raster, not stale overlapping revisions.
            assert_eq!(previous.image_id, current.image_id);
        }
        identity = Some(current);
    }
    let full = texture?;
    assert!((area - full.size.width.as_f32() * full.size.height.as_f32()).abs() < 0.1, "the native slices cover the complete uncropped figure exactly once");
    identity
}
