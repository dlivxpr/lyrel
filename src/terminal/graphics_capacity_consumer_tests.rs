// Deliberately ignored: this allocates the real 120 x 1080p workload and starts
// Node in real PTYs. It verifies the production consumer, not native GPU timing.
struct CapacityConsumer {
    pane: u64,
    view: Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    serial: u64,
    image_id: u32,
    manifest: std::path::PathBuf,
}

fn capacity_wait(cx: &mut TestAppContext, reason: &str, mut ready: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while !ready(cx) {
        assert!(std::time::Instant::now() < deadline, "full-size production consumer: {reason}");
        graphics_consumer_progress(cx);
    }
}

impl CapacityConsumer {
    fn spawn(daemon: &GraphicsDaemonProcess, pane_index: u32, root: &std::path::Path, cx: &mut TestAppContext) -> Self {
        let manifest = root.join(format!("pane-{pane_index}.json"));
        let shell = crate::daemon::protocol::ShellSpec {
            program: "node".into(),
            args: vec![
                concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/kitty-capacity.mjs").into(),
                "--pane".into(), pane_index.to_string(),
                "--manifest".into(), manifest.to_string_lossy().into_owned(),
            ],
            args_are_lyrel_defaults: false,
        };
        let session = daemon.panes().spawn(
            Some(root.to_path_buf()),
            crate::daemon::protocol::WinSize { cols: 80, rows: 24, cell_w: 10, cell_h: 20 },
            Some(shell), None, None,
        ).unwrap();
        let pane = session.pane_id();
        session.detach().unwrap();
        let stream = daemon.stream_with_capacity(pane, 512 << 20, 512 << 20);
        let (view, window) = graphics_build_rooted_consumer(cx, pane, stream, Vec::new());
        capacity_wait(cx, "Node real-PTY readiness", |cx| view.read_with(cx, |view, _| view.scrollback_text().contains("CAP146:0:ready:0:0")));
        graphics_assert_production_paint(window, &view, cx);
        Self { pane, view, window, serial: 0, image_id: 146000 + pane_index, manifest }
    }

    fn command(&mut self, command: &str, cx: &mut TestAppContext) -> (u64, u64) {
        self.serial += 1;
        let phase = command.split_whitespace().next().unwrap();
        let prefix = format!("CAP146:{}:{phase}:", self.serial);
        self.view.read_with(cx, |view, _| view.terminal.write(format!("{command}\r").into_bytes()));
        let mut counters = None;
        capacity_wait(cx, command, |cx| {
            self.view.read_with(cx, |view, _| {
                let text = view.scrollback_text();
                assert!(!text.contains(&format!("CAP146:{}:failed:", self.serial)), "fixture command failed: {text}");
                counters = text.lines().find_map(|line| {
                    let suffix = &line[line.find(&prefix)? + prefix.len()..];
                    let mut fields = suffix.split(|character: char| character == ':' || character.is_whitespace());
                    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
                });
                counters.is_some()
            })
        });
        counters.unwrap()
    }

    fn wait_loaded(&self, cx: &mut TestAppContext) {
        capacity_wait(cx, "all sixty actual decoded frame owners", |cx| self.view.read_with(cx, |view, _| {
            let usage = view.terminal.images().allocation_observation();
            usage["decodedFrames"] == 60 && usage["pendingOffers"] == 0 && usage["sourceBytes"] == 0 && usage["scratchBytes"] == 0
        }));
    }

    fn selected(&self, cx: &mut TestAppContext) -> crate::terminal::images::RenderPlacement {
        self.view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            let image = images.into_iter().find(|image| image.placement.image_id == self.image_id).expect("full-size placement visible");
            assert!(!image.evicted);
            assert_eq!((image.width_px, image.height_px), (1920, 1080));
            image
        })
    }

    fn restore(self, daemon: &GraphicsDaemonProcess, cx: &mut TestAppContext, frames: usize) -> Self {
        let Self { pane, view, window, serial, image_id, manifest } = self;
        // End old derived ownership before the complete replay's peak offers.
        graphics_recovery_destroy(view, window, cx);
        let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream_with_capacity(pane, 512 << 20, 512 << 20), Vec::new());
        let mut restored = Self { pane, view, window, serial, image_id, manifest };
        capacity_wait(cx, "fresh replay retains exactly the accepted frame owners", |cx| restored.view.read_with(cx, |view, _| {
            let usage = view.terminal.images().allocation_observation();
            usage["decodedFrames"] == frames && usage["pendingOffers"] == 0 && usage["sourceBytes"] == 0 && usage["scratchBytes"] == 0
        }));
        restored.command("stats", cx);
        graphics_assert_production_paint(restored.window, &restored.view, cx);
        restored
    }
}

fn capacity_rgba_digest(image: &gpui_kit::RenderImage) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hash = Sha256::new();
    let mut converted = [0u8; 8192];
    for chunk in image.as_bytes(0).unwrap().chunks(converted.len()) {
        converted[..chunk.len()].copy_from_slice(chunk);
        for pixel in converted[..chunk.len()].chunks_exact_mut(4) { pixel.swap(0, 2); }
        hash.update(&converted[..chunk.len()]);
    }
    let mut result = String::with_capacity(64);
    for byte in hash.finalize() { write!(&mut result, "{byte:02x}").unwrap(); }
    result
}

fn capacity_barcode(image: &gpui_kit::RenderImage) -> u8 {
    let bytes = image.as_bytes(0).unwrap();
    let mut id = 0;
    for bit in 0..8 {
        let offset = 4 * (48 * 1920 + 48 + 32 * bit);
        let pixel = &bytes[offset..offset + 4];
        assert!(pixel == [0, 0, 0, 255] || pixel == [255, 255, 255, 255]);
        id = (id << 1) | u8::from(pixel[0] == 255);
    }
    id
}

#[gpui_kit::test]
#[ignore = "real two-pane 120 distinct 1080p workload; central capacity acceptance, not native present proof"]
fn graphics_consumer_fullsize_two_panes_retain_reuse_restore_mutate_and_release(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    let daemon = GraphicsDaemonProcess::start();
    let root = tempfile::tempdir().unwrap();
    let mut consumers: Vec<_> = (0..2).map(|pane| CapacityConsumer::spawn(&daemon, pane, root.path(), cx)).collect();
    let mut identities = std::collections::BTreeSet::new();
    let mut versions = Vec::new();
    for consumer in &mut consumers {
        assert_eq!(consumer.command("load", cx).0, 60);
        consumer.wait_loaded(cx);
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&consumer.manifest).unwrap()).unwrap();
        let records = manifest["records"].as_array().expect("source manifest records");
        assert_eq!(records.len(), 60);
        let mut retained = std::collections::BTreeMap::new();
        for frame in 0..60 {
            consumer.command(&format!("select {}", frame + 1), cx);
            let image = consumer.selected(cx);
            let data = image.data.as_ref().unwrap();
            let digest = capacity_rgba_digest(data);
            let expected = records.iter().find(|record| record["frameId"] == frame).unwrap();
            assert_eq!(digest, expected["sha256"].as_str().unwrap());
            assert!(identities.insert(digest), "both panes must contain sixty globally distinct full-resolution sources");
            assert_eq!(capacity_barcode(data), frame as u8);
            retained.insert(frame as u8, std::sync::Arc::downgrade(data));
        }
        eprintln!("full-size consumer loaded pane={}: {}", consumer.pane, consumer.view.read_with(cx, |view, _| view.terminal.images().allocation_observation()));
        versions.push(retained);
    }
    assert_eq!(identities.len(), 120);
    for consumer in &mut consumers { assert_eq!(consumer.command("start", cx).0, 60); }
    let mut seen = [std::collections::BTreeMap::<u8, u32>::new(), std::collections::BTreeMap::<u8, u32>::new()];
    let mut previous = [None, None];
    capacity_wait(cx, "both already-loaded animations reuse all sixty decoded versions", |cx| {
        for (index, consumer) in consumers.iter().enumerate() {
            let image = consumer.selected(cx);
            let data = image.data.as_ref().unwrap();
            let frame = capacity_barcode(data);
            assert!(std::sync::Arc::ptr_eq(data, &versions[index][&frame].upgrade().unwrap()), "playback rebuilt a retained pixel version");
            if previous[index] != Some(frame) {
                *seen[index].entry(frame).or_default() += 1;
                previous[index] = Some(frame);
            }
        }
        seen.iter().all(|frames| frames.len() == 60 && frames.values().all(|visits| *visits >= 2))
    });
    // These are authority/reuse observations only, deliberately not an FPS claim.
    for consumer in &mut consumers {
        consumer.command("stop", cx);
        let baseline = consumer.command("stats", cx);
        graphics_assert_production_paint(consumer.window, &consumer.view, cx);
        assert_eq!(consumer.command("stats", cx), baseline, "steady source must not upload more pixels/APCs");
        consumer.view.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    }
    for consumer in &consumers { graphics_playback_wait(&consumer.view, cx, |playback| playback.paused); }
    for consumer in &mut consumers {
        consumer.view.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(true));
        graphics_playback_wait(&consumer.view, cx, |playback| !playback.paused);
        consumer.command("input synthetic-capacity-146", cx);
        consumer.command("edit 1", cx);
        consumer.command("compose 1 2", cx);
        consumer.wait_loaded(cx);
    }
    consumers = consumers.into_iter().map(|consumer| consumer.restore(&daemon, cx, 60)).collect();
    for consumer in &mut consumers {
        consumer.command("replace", cx);
        consumer.wait_loaded(cx);
        consumer.command("delete", cx);
        capacity_wait(cx, "intentional delete releases all real owners without eviction notice", |cx| {
            consumer.window.update(cx, |_, window, cx| consumer.view.read(cx).terminal.images().release_retired(window, cx)).unwrap();
            consumer.view.read_with(cx, |view, _| {
                let store = view.terminal.images();
                let usage = store.allocation_observation();
                assert!(graphics_visible_images(view).iter().all(|image| !image.evicted));
                usage["cacheBytes"] == 0 && usage["sourceBytes"] == 0 && usage["metadataBytes"] == 0
            })
        });
    }
    consumers = consumers.into_iter().map(|consumer| consumer.restore(&daemon, cx, 0)).collect();
    for consumer in &consumers {
        assert!(consumer.view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()), "intentional deletion cannot revive pictures or historical notices after replay");
    }
    for consumer in consumers {
        daemon.panes().kill(consumer.pane).unwrap();
        graphics_recovery_destroy(consumer.view, consumer.window, cx);
    }
}
