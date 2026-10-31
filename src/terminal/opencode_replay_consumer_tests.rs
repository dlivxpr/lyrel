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
            "OpenCode {description}; child_exited={}; instances={:#?}; graphics={}; text={}",
            view.read_with(cx, |view, _| view.terminal.child_exited()),
            view.read_with(cx, |view, _| native_pictures(view, 1.)),
            view.read_with(cx, |view, _| view.terminal.images().recovery_diagnostics()),
            view.read_with(cx, |view, _| view.scrollback_text()));
        graphics_consumer_progress(cx);
    }
}

// Colors are independent BGRA literals from the fixture's published legend.
// Keep one record per production placement: a color set or union rectangle
// hides duplicate placements, including two copies at exactly the same origin.
#[derive(Debug)]
struct NativePicture {
    color: [u8; 4],
    placement: lyrel_core::core::kitty_graphics::Placement,
    // Cell coordinates, normalized from the production paint rectangle.
    bounds: [f32; 4],
    painted: bool,
    evicted: bool,
}

fn native_pictures(view: &TerminalView, scale: f32) -> Vec<NativePicture> {
    let offset = view.terminal.term.lock().display_offset() as i32;
    let (cell_width, line_height) = view.cell_geom
        .map(|geom| (geom.cell_width, geom.line_height))
        .unwrap_or((px(10.), px(20.)));
    graphics_visible_images(view).into_iter().filter_map(|image| {
        let (bounds, _) = image.paint_bounds(point(px(0.), px(0.)), cell_width, line_height, 0, offset, scale);
        let bytes = image.data.as_ref()?.as_bytes(0)?;
        let mut counts = std::collections::BTreeMap::new();
        for pixel in bytes.chunks_exact(4) {
            *counts.entry([pixel[0], pixel[1], pixel[2], pixel[3]]).or_insert(0usize) += 1;
        }
        let color = counts.into_iter().max_by_key(|(_, count)| *count)?.0;
        Some(NativePicture {
            color,
            placement: image.placement,
            bounds: [bounds.origin.x.as_f32() / cell_width.as_f32(), bounds.origin.y.as_f32() / line_height.as_f32(),
                (bounds.origin.x + bounds.size.width).as_f32() / cell_width.as_f32(),
                (bounds.origin.y + bounds.size.height).as_f32() / line_height.as_f32()],
            painted: image.painted.load(std::sync::atomic::Ordering::Acquire),
            evicted: image.evicted,
        })
    }).collect()
}

fn native_in_viewport(picture: &NativePicture) -> bool {
    let [left, top, right, bottom] = picture.bounds;
    left < 120. && right > 0. && top < 60. && bottom > 0.
}

fn native_colors(view: &TerminalView) -> std::collections::BTreeSet<[u8; 4]> {
    native_pictures(view, 1.).iter().filter(|picture| native_in_viewport(picture))
        .map(|picture| picture.color).collect()
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
    let scale = window.update(cx, |_, window, _| window.scale_factor()).unwrap();
    view.read_with(cx, |view, _| {
        let all = native_pictures(view, scale);
        let pictures: Vec<_> = all.iter().filter(|picture| native_in_viewport(picture)).collect();
        let diagnostics = format!("old={old}; all_instances={all:#?}; graphics={}; text={:?}",
            view.terminal.images().recovery_diagnostics(), view.scrollback_text());
        eprintln!("OpenCode native card diagnostics: {diagnostics}");
        assert_eq!(pictures.len(), 6, "exactly six visible instances, not six distinct colors; {diagnostics}");
        let mut cards = Vec::new();
        for color in palette {
            let instances: Vec<_> = pictures.iter().copied().filter(|picture| picture.color == color).collect();
            assert_eq!(instances.len(), 1, "each inline picture has one actual placement instance, including coincident duplicates; color={color:?}; {diagnostics}");
            cards.push(instances[0]);
        }
        // Independent v2.2.0 SessionImages contract: height=clamp(rows/4,4,8),
        // width=2*height, gap=1. At 120x60 no flex shrink is needed. The +1 more
        // text is an independent grid witness of the native card container.
        let term = view.terminal.term.lock();
        let offset = term.display_offset() as i32;
        let mut more = Vec::new();
        for row in 0..60i32 {
            let text: String = term.grid[Line(row - offset)][..].iter().map(|cell| cell.c()).collect();
            if let Some(byte) = text.find("+1 more") {
                more.push((text[..byte].chars().count() as f32, row as f32));
            }
        }
        drop(term);
        assert_eq!(more.len(), 2, "one +1 more container witness per user/tool card; {diagnostics}");
        for (card, marker) in cards.chunks_exact(3).zip(more) {
            let [left, middle, right] = [card[0].bounds, card[1].bounds, card[2].bounds];
            let start_col = card[0].placement.anchor_col;
            let start_row = card[0].placement.anchor_row;
            for (index, picture) in card.iter().enumerate() {
                assert_eq!((picture.placement.cols, picture.placement.rows), (16, 8), "native 16x8 card slot; {diagnostics}");
                assert_eq!((picture.placement.anchor_col, picture.placement.anchor_row),
                    (start_col + 17 * index as i64, start_row), "source order and one-cell native gap; {diagnostics}");
                let [x0, y0, x1, y1] = picture.bounds;
                let slot_left = (start_col + 17 * index as i64) as f32;
                assert!(x0 >= slot_left && x1 <= slot_left + 16. && y0 >= left[1] && y1 <= left[1] + 8.,
                    "actual picture stays inside its native container slot; {diagnostics}");
                assert!(x0 >= 0. && y0 >= 0. && x1 <= 120. && y1 <= 60. && x1 > x0 && y1 > y0,
                    "actual picture stays inside the viewport; {diagnostics}");
                assert!(!picture.evicted, "upstream native deletion must not become eviction; {diagnostics}");
                assert!(picture.painted, "production TerminalElement must claim each decoded picture; {diagnostics}");
            }
            assert_eq!(left[1], middle[1], "native card row alignment; {diagnostics}");
            assert_eq!(middle[1], right[1], "native card row alignment; {diagnostics}");
            assert!(left[2] <= middle[0] && middle[2] <= right[0], "native card pictures must not overlap; {diagnostics}");
            assert!(marker.0 >= start_col as f32 + 51. && marker.0 <= start_col as f32 + 52.
                && marker.1 >= left[1] + 3. && marker.1 <= left[1] + 4.,
                "pictures and +1 more occupy the same official native container; marker={marker:?}; {diagnostics}");
            assert!(start_col >= 0 && start_col as f32 + 51. + 8. + 2. <= 120.,
                "complete native card including +1 more and right padding fits the viewport; {diagnostics}");
        }
        assert!(cards[0].bounds[3] <= cards[3].bounds[1], "user picture card must precede the separate tool card; {diagnostics}");
        for (index, first) in pictures.iter().enumerate() {
            for second in &pictures[index + 1..] {
                let [a0, a1, a2, a3] = first.bounds;
                let [b0, b1, b2, b3] = second.bounds;
                assert!(a2 <= b0 || b2 <= a0 || a3 <= b1 || b3 <= a1,
                    "every actual picture pair is disjoint, including across cards; {diagnostics}");
            }
        }
    });
}

#[gpui_kit::test]
fn graphics_consumer_static_placements_count_coincident_instances_and_replace_by_id(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    // The same public static protocol used by OpenTUI: upload independently,
    // position with CUP, and display with explicit image/placement IDs and C=1.
    graphics_send_fixture(&view, cx, 1,
        b"\x1b[2J\x1b[3J\x1b_Ga=t,i=70,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[3;2H\x1b_Ga=p,i=70,p=1,c=4,r=2,C=1,q=2\x1b\\\x1b[3;8H\x1b_Ga=p,i=70,p=2,c=4,r=2,C=1,q=2\x1b\\");
    graphics_layout_wait_image(&view, cx, 70);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let pictures = native_pictures(view, 1.);
        assert_eq!(pictures.len(), 2, "identical decoded color must not collapse independent placement instances");
        assert!(pictures.iter().all(|picture| picture.color == [0, 0, 255, 255]));
        assert_eq!(pictures.iter().map(|picture| picture.placement.placement_id).collect::<Vec<_>>(), [1, 2]);
    });
    // Replacing p=2 must keep its instance count, whereas a distinct p=3 at
    // p=1's exact origin remains a separate instance despite equal color/bounds.
    graphics_send_fixture(&view, cx, 2,
        b"\x1b[3;8H\x1b_Ga=p,i=70,p=2,c=4,r=2,C=1,q=2\x1b\\\x1b[3;2H\x1b_Ga=p,i=70,p=3,c=4,r=2,C=1,q=2\x1b\\");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let pictures = native_pictures(view, 1.);
        assert_eq!(pictures.len(), 3, "coincident pictures are real instances, not a union bounding box");
        assert_eq!(pictures.iter().map(|picture| picture.placement.placement_id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(pictures[0].bounds, pictures[2].bounds);
        assert_ne!(pictures[0].placement.key, pictures[2].placement.key);
    });
    graphics_send_fixture(&view, cx, 3, b"\x1b_Ga=d,d=i,i=70,p=2,q=2\x1b\\");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(native_pictures(view, 1.).iter().map(|picture| picture.placement.placement_id).collect::<Vec<_>>(), [1, 3]);
    });
    graphics_send_fixture(&view, cx, 4, b"\x1b_Ga=d,d=I,i=70,q=2\x1b\\");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
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
