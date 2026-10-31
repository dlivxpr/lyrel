// This file is included beside the existing production graphics consumer harness.
struct OmpReplayFixture {
    root: tempfile::TempDir,
}

impl OmpReplayFixture {
    fn prepare(case: &str, mode: &str) -> Self {
        let omp = std::env::var_os("LYREL_TEST_OMP")
            .expect("set LYREL_TEST_OMP to an installed released omp executable");
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("node")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/omp-native-replay.mjs"))
            .args(["prepare", "--root"])
            .arg(root.path().join("owned"))
            .arg("--omp").arg(omp)
            .args(["--case", case, "--mode", mode])
            .status().unwrap();
        assert!(status.success(), "preparing public synthetic omp history");
        Self { root }
    }

    fn spawn(&self, daemon: &GraphicsDaemonProcess) -> u64 {
        let shell = crate::daemon::protocol::ShellSpec {
            program: "node".into(),
            args: vec![
                concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/omp-native-replay.mjs").into(),
                "run".into(), "--wait-ready".into(), "--root".into(),
                self.root.path().join("owned").to_string_lossy().into_owned(),
            ],
            args_are_lyrel_defaults: false,
        };
        let mut session = daemon.panes().spawn(
            Some(self.root.path().to_path_buf()),
            crate::daemon::protocol::WinSize { cols: 80, rows: 24, cell_w: 10, cell_h: 20 },
            Some(shell), None, None,
        ).unwrap();
        let pane = session.pane_id();
        session.detach().unwrap();
        pane
    }

    fn finish(&self, view: &Entity<TerminalView>, cx: &mut TestAppContext) {
        view.read_with(cx, |view, _| view.terminal.write(b"/exit\r".to_vec()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let audit = self.root.path().join("owned/audit.json");
        while !audit.exists() {
            assert!(std::time::Instant::now() < deadline, "native omp did not exit and audit");
            graphics_consumer_progress(cx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let audit: serde_json::Value = serde_json::from_slice(&std::fs::read(audit).unwrap()).unwrap();
        assert_eq!(audit["zeroInference"], true, "{audit}");
        assert_eq!(audit["exitCode"], 0, "{audit}");
        eprintln!("released omp replay evidence: {audit}");
    }
}

impl Drop for OmpReplayFixture {
    fn drop(&mut self) {
        // A failed assertion must not strand this fixture's native TUI child.
        let active = self.root.path().join("owned/running.json");
        let Ok(bytes) = std::fs::read(active) else { return; };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return; };
        let Some(pid) = value["pid"].as_u64() else { return; };
        if cfg!(windows) {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"]).status();
        } else {
            let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
        }
    }
}

fn omp_replay_scan(
    view: &Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    cx: &mut TestAppContext,
) -> std::collections::BTreeMap<Vec<u8>, (u32, u32, i64, usize)> {
    let mut found = std::collections::BTreeMap::new();
    let history = view.read_with(cx, |view, _| view.terminal.term.lock().grid.history_size());
    view.update(cx, |view, cx| view.scroll(-100_000, None, &Modifiers::default(), cx));
    for _ in 0..=(history / 12 + 2) {
        graphics_assert_production_paint(window, view, cx);
        view.read_with(cx, |view, _| {
            for image in graphics_visible_images(view) {
                let Some(data) = &image.data else { continue; };
                let bytes = data.as_bytes(0).unwrap();
                if bytes.len() == 64 * 32 * 4 {
                    assert_eq!(&bytes[(16 * 64 + 32) * 4..][..4], &[0, 0, 0, 255]);
                    found.insert(bytes[..4].to_vec(), (image.placement.image_id, image.placement.placement_id, image.anchor_row, image.anchor_col));
                } else if bytes.chunks_exact(4).any(|pixel| pixel == [86, 52, 18, 255])
                    && bytes.chunks_exact(4).any(|pixel| pixel == [0, 128, 255, 255])
                {
                    found.insert(b"rasterized-svg".to_vec(), (image.placement.image_id, image.placement.placement_id, image.anchor_row, image.anchor_col));
                }
                if image.painted.load(std::sync::atomic::Ordering::Acquire) {
                    let (crop, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), history, 0, 1.);
                    assert!(crop.size.width > px(0.) && crop.size.height > px(0.));
                }
            }
        });
        view.update(cx, |view, cx| view.scroll(12, None, &Modifiers::default(), cx));
    }
    found
}

#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP, Node and a real released omp; never calls a model"]
fn graphics_consumer_released_omp_native_resume_pictures_and_budget(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    for case in ["pictures", "budget"] {
        let fixture = OmpReplayFixture::prepare(case, "placeholders");
        let daemon = GraphicsDaemonProcess::start();
        let pane = fixture.spawn(&daemon);
        let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| view.terminal.write(b"LYREL_OMP_START\r".to_vec()));
        let expected = if case == "budget" { 8 } else { 3 };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let found = loop {
            graphics_consumer_progress(cx);
            let found = omp_replay_scan(&view, window, cx);
            if found.len() == expected && found.contains_key(b"rasterized-svg".as_slice()) { break found; }
            assert!(std::time::Instant::now() < deadline, "missing actual decoded native pictures: {found:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(found.contains_key([192,128,240,255].as_slice()), "native tool-result pixels");
        if case == "pictures" {
            let assistant = found.get([0,0,255,255].as_slice()).unwrap();
            let tool = found.get([192,128,240,255].as_slice()).unwrap();
            let figure = found.get(b"rasterized-svg".as_slice()).unwrap();
            assert_ne!(assistant.0, tool.0);
            assert_ne!(tool.0, figure.0);
            assert!(assistant.2 < tool.2 && tool.2 < figure.2, "ordered native image placements: {found:?}");
        } else {
            for color in [[0,0,255,255],[0,255,0,255],[255,0,0,255],[0,255,255,255]] {
                assert!(!found.contains_key(color.as_slice()), "upstream d=I demotion must not revive old image");
            }
            for color in [[255,0,255,255],[255,255,0,255],[0,96,192,255],[192,0,96,255],[96,128,0,255],[128,128,128,255]] {
                assert!(found.contains_key(color.as_slice()), "wrong recent native image identity: {found:?}");
            }
            assert_eq!(omp_replay_scan(&view, window, cx), found, "scroll-out/back must preserve deletion and exact placement identity");
        }
        fixture.finish(&view, cx);
        graphics_recovery_destroy(view, window, cx);
    }
}

#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP; real native disabled-policy resume"]
fn graphics_consumer_released_omp_native_disabled_policy(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    for case in ["disabled", "protocol-off"] {
        let fixture = OmpReplayFixture::prepare(case, "placeholders");
        let daemon = GraphicsDaemonProcess::start();
        let pane = fixture.spawn(&daemon);
        let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| view.terminal.write(b"LYREL_OMP_START\r".to_vec()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            graphics_consumer_progress(cx);
            let ready = view.read_with(cx, |view, _| {
                view.scrollback_text().contains("LYREL_REPLAY_END")
            });
            if ready { break; }
            assert!(std::time::Instant::now() < deadline, "native disabled replay did not finish");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(omp_replay_scan(&view, window, cx).is_empty(), "explicit disabled is authoritative");
        fixture.finish(&view, cx);
        graphics_recovery_destroy(view, window, cx);
    }
}
