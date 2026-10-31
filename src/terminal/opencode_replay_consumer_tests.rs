use super::*;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/opencode-image-replay.mjs");

struct OpenCodeReplay {
    root: tempfile::TempDir,
}

impl OpenCodeReplay {
    fn prepare(previews: &str) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let output = std::process::Command::new("node")
            .args([SCRIPT, "--prepare"])
            .arg(root.path())
            .args(["--previews", previews])
            .output()
            .expect("Node.js and released OpenCode 2.2.0 must be installed; never skip a missing native prerequisite");
        assert!(
            output.status.success(),
            "native fixture preparation failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Self { root }
    }

    fn spawn(&self, daemon: &GraphicsDaemonProcess) -> u64 {
        use crate::daemon::protocol::{ShellSpec, WinSize};
        let session = daemon.panes().spawn(
            None,
            WinSize { cols: 120, rows: 60, cell_w: 10, cell_h: 20 },
            Some(ShellSpec {
                program: "node".into(),
                args: vec![SCRIPT.into(), "--resume".into(), self.root.path().to_string_lossy().into_owned()],
                args_are_lyrel_defaults: false,
            }),
            None,
            None,
        ).unwrap();
        let pane = session.pane_id();
        session.detach().unwrap();
        pane
    }
}

fn native_wait(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    description: &str,
    ready: impl Fn(&TerminalView) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        if view.read_with(cx, |view, _| ready(view)) { return; }
        assert!(std::time::Instant::now() < deadline,
            "OpenCode {description}; child_exited={}; text={}",
            view.read_with(cx, |view, _| view.terminal.child_exited()),
            view.read_with(cx, |view, _| view.scrollback_text()));
        graphics_consumer_progress(cx);
    }
}

// Colors are independent literals from the fixture's published picture legend.
// Read production-decoded pixels, not APC counts, text labels, or source checks.
fn native_colors(view: &TerminalView) -> std::collections::BTreeSet<[u8; 4]> {
    let offset = view.terminal.term.lock().display_offset() as i32;
    graphics_visible_images(view).iter().filter_map(|image| {
        let (bounds, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, offset, 1.);
        if bounds.origin.y >= px(1200.) || bounds.origin.y + bounds.size.height <= px(0.) { return None; }
        let bytes = image.data.as_ref()?.as_bytes(0)?;
        let mut counts = std::collections::BTreeMap::new();
        for pixel in bytes.chunks_exact(4) {
            let color = [pixel[0], pixel[1], pixel[2], pixel[3]];
            *counts.entry(color).or_insert(0usize) += 1;
        }
        counts.into_iter().max_by_key(|(_, count)| *count).map(|(color, _)| color)
    }).collect()
}

fn native_assert_cards(view: &Entity<TerminalView>, window: gpui_kit::AnyWindowHandle, cx: &mut TestAppContext, old: bool) {
    let palette = if old {
        [[0, 0, 220, 255], [0, 180, 0, 255], [220, 0, 0, 255],
         [180, 180, 0, 255], [0, 180, 180, 255], [0, 100, 220, 255]]
    } else {
        [[70, 70, 240, 255], [70, 210, 70, 255], [240, 70, 70, 255],
         [210, 210, 70, 255], [70, 210, 210, 255], [70, 140, 240, 255]]
    };
    let expected = palette.into_iter().collect::<std::collections::BTreeSet<_>>();
    native_wait(view, cx, "expected six inline pictures after native navigation", |view| native_colors(view) == expected);
    graphics_assert_production_paint_at(window, view, cx, TermSize::new(120, 60));
    view.read_with(cx, |view, _| {
        assert_eq!(native_colors(view), expected, "three per user/tool card, not the fourth magenta/violet picture");
        let offset = view.terminal.term.lock().display_offset() as i32;
        let mut boxes = std::collections::BTreeMap::<[u8; 4], [f32; 4]>::new();
        for image in graphics_visible_images(view) {
            let (bounds, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, offset, 1.);
            if bounds.origin.y >= px(1200.) || bounds.origin.y + bounds.size.height <= px(0.) { continue; }
            assert!(!image.evicted, "upstream native deletion must not become eviction");
            assert!(image.painted.load(std::sync::atomic::Ordering::Acquire), "production TerminalElement must claim the decoded picture");
            let mut counts = std::collections::BTreeMap::new();
            for pixel in image.data.as_ref().unwrap().as_bytes(0).unwrap().chunks_exact(4) {
                *counts.entry([pixel[0], pixel[1], pixel[2], pixel[3]]).or_insert(0usize) += 1;
            }
            let color = counts.into_iter().max_by_key(|(_, count)| *count).unwrap().0;
            let rectangle = [bounds.origin.x.as_f32(), bounds.origin.y.as_f32(),
                (bounds.origin.x + bounds.size.width).as_f32(), (bounds.origin.y + bounds.size.height).as_f32()];
            boxes.entry(color).and_modify(|old| {
                old[0] = old[0].min(rectangle[0]); old[1] = old[1].min(rectangle[1]);
                old[2] = old[2].max(rectangle[2]); old[3] = old[3].max(rectangle[3]);
            }).or_insert(rectangle);
        }
        for card in palette.chunks_exact(3) {
            let [left, middle, right] = [boxes[&card[0]], boxes[&card[1]], boxes[&card[2]]];
            assert_eq!(left[1], middle[1]); assert_eq!(middle[1], right[1]);
            assert!(left[2] <= middle[0] && middle[2] <= right[0], "native card pictures must not overlap");
        }
        assert!(boxes[&palette[0]][3] <= boxes[&palette[3]][1], "user picture card must precede the separate tool card");
    });
}

#[gpui_kit::test]
#[ignore = "real released OpenCode 2.2.0 + Node.js; isolated PTY/daemon and production rooted consumer"]
fn opencode_native_saved_images_paginate_remount_delete_without_inference(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let fixture = OpenCodeReplay::prepare("on");
    let daemon = GraphicsDaemonProcess::start();
    let pane = fixture.spawn(&daemon);
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    let size = crate::daemon::protocol::WinSize { cols: 120, rows: 60, cell_w: 10, cell_h: 20 };
    let (view, window) = graphics_build_rooted_consumer_at(cx, pane,
        daemon.stream_with_capacity_at(pane, 64 << 20, 128 << 20, size), Vec::new(), TermSize::new(120, 60),
        if cfg!(windows) { PtySource::LocalConpty } else { PtySource::Raw });
    native_assert_cards(&view, window, cx, false);
    view.read_with(cx, |view, _| view.terminal.write(b"\x1b[5~".repeat(4)));
    native_wait(&view, cx, "native PageUp scrolls both cards out of the viewport", |view| {
        native_colors(view).is_empty() && view.scrollback_text().contains("PAGE_")
    });
    view.read_with(cx, |view, _| view.terminal.write(b"\x1b[F".to_vec()));
    native_assert_cards(&view, window, cx, false);
    // Native session.first loads earlier HTTP pages and mounts older rows.
    view.read_with(cx, |view, _| view.terminal.write(b"\x07".to_vec()));
    native_assert_cards(&view, window, cx, true);
    view.read_with(cx, |view, _| view.terminal.write(b"\x1b[F".to_vec()));
    native_assert_cards(&view, window, cx, false);
    // These F6/F7 overrides are public cli.json bindings, not agent hooks.
    // Closing the native tab unmounts its image nodes and emits deletions.
    view.read_with(cx, |view, _| view.terminal.write(b"\x1b[17~".to_vec()));
    native_wait(&view, cx, "tab close deletes all placements", |view| graphics_visible_images(view).is_empty());
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(120, 60));
    view.read_with(cx, |view, _| view.terminal.write(b"\x1b[18~".to_vec()));
    native_assert_cards(&view, window, cx, false);
    view.read_with(cx, |view, _| view.terminal.write(vec![3]));
    native_wait(&view, cx, "public export proves unchanged saved transcript and zero usage", |view| {
        view.terminal.child_exited() && view.scrollback_text().contains("OPENCODE_REPLAY_VERIFIED")
    });
    native_wait(&view, cx, "native exit deletes images", |view| graphics_visible_images(view).is_empty());
    graphics_recovery_destroy(view, window, cx);
    // Fresh decoder/window must not resurrect upstream-deleted pixels. This is
    // deliberately separate from resume, which retransmits native graphics.
    let (fresh, fresh_window) = graphics_build_rooted_consumer_at(cx, pane,
        daemon.stream_with_capacity_at(pane, 64 << 20, 128 << 20, size), Vec::new(), TermSize::new(120, 60),
        if cfg!(windows) { PtySource::LocalConpty } else { PtySource::Raw });
    graphics_assert_production_paint_at(fresh_window, &fresh, cx, TermSize::new(120, 60));
    assert!(fresh.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(fresh, fresh_window, cx);
}

#[gpui_kit::test]
#[ignore = "real released OpenCode 2.2.0 + Node.js; disabled public preview policy must remain disabled"]
fn opencode_native_disabled_previews_remain_text(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let fixture = OpenCodeReplay::prepare("off");
    let daemon = GraphicsDaemonProcess::start();
    let pane = fixture.spawn(&daemon);
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    let size = crate::daemon::protocol::WinSize { cols: 120, rows: 60, cell_w: 10, cell_h: 20 };
    let (view, window) = graphics_build_rooted_consumer_at(cx, pane,
        daemon.stream_with_capacity_at(pane, 64 << 20, 128 << 20, size), Vec::new(), TermSize::new(120, 60),
        if cfg!(windows) { PtySource::LocalConpty } else { PtySource::Raw });
    native_wait(&view, cx, "disabled tail transcript mounted", |view| view.scrollback_text().contains("TAIL_TOOL"));
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(120, 60));
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    view.read_with(cx, |view, _| view.terminal.write(b"\x07".to_vec()));
    native_wait(&view, cx, "disabled old transcript paginated", |view| view.scrollback_text().contains("OLD_USER"));
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(120, 60));
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    view.read_with(cx, |view, _| view.terminal.write(vec![3]));
    native_wait(&view, cx, "disabled replay exports unchanged zero-usage transcript", |view| {
        view.terminal.child_exited() && view.scrollback_text().contains("OPENCODE_REPLAY_VERIFIED")
    });
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}
