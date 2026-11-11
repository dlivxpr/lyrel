// Native upstream components own rendering and deletion; this consumer never
// sends Kitty commands and does not claim an OS clipboard/full CLI paste test.
struct NativeStaticFixture {
    daemon: GraphicsDaemonProcess,
    pane: u64,
}

impl NativeStaticFixture {
    fn start(agent: crate::core::cli_agent::CLIAgent, policy: &str) -> Self {
        let key = match agent {
            crate::core::cli_agent::CLIAgent::Kimi => "LYREL_KIMI_GRAPHICS_SOURCE",
            crate::core::cli_agent::CLIAgent::Hermes => "LYREL_HERMES_GRAPHICS_SOURCE",
            other => panic!("unsupported native static agent: {}", other.slug()),
        };
        let source = std::env::var(key).unwrap_or_else(|_| panic!("set {key} to an isolated pinned upstream source checkout with native dependencies"));
        let daemon = GraphicsDaemonProcess::start();
        let shell = native_graphics_scoped_shell(agent, crate::daemon::protocol::ShellSpec {
            program: "node".into(),
            args: vec![concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/native-static-images.mjs").into(),
                "--agent".into(), agent.slug().into(), "--source".into(), source,
                "--policy".into(), policy.into(), "--temp-parent".into(), daemon.dir.path().to_string_lossy().into_owned()],
            args_are_lyrel_defaults: false,
        });
        let session = daemon.panes().spawn(None, crate::daemon::protocol::WinSize {
            cols: 80, rows: 24, cell_w: 10, cell_h: 20,
        }, Some(shell), None, None).unwrap();
        let pane = session.pane_id();
        session.detach().unwrap();
        Self { daemon, pane }
    }
}

impl Drop for NativeStaticFixture {
    fn drop(&mut self) { let _ = self.daemon.panes().kill(self.pane); }
}

fn native_static_exercise(cx: &mut TestAppContext, agent: crate::core::cli_agent::CLIAgent) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    for policy in ["auto", "off"] {
        let fixture = NativeStaticFixture::start(agent, policy);
        let (view, window) = graphics_build_rooted_consumer(cx, fixture.pane, fixture.daemon.stream(fixture.pane), Vec::new());
        for (phase, pixels) in [("ATTACHMENT", NATIVE_PICTURE_FIRST), ("REMOUNT", NATIVE_PICTURE_FIRST), ("REPLACEMENT", NATIVE_PICTURE_LAST)] {
            native_picture_wait(&view, cx, phase, |view| {
                view.scrollback_text().contains(&format!("NATIVE_STATIC_{phase}"))
                    && (policy == "off" || native_picture_pixels(view, pixels).is_some())
            });
            graphics_assert_production_paint(window, &view, cx);
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                if policy == "off" { assert!(images.is_empty(), "explicit native off must remain authoritative"); }
                else {
                    assert!(!images.is_empty());
                    let image = native_picture_pixels(view, pixels).unwrap();
                    assert!(images.iter().all(|slice| slice.placement.image_id == image.placement.image_id),
                        "only the native thumbnail or eligible local image; no SVG/URL rendering or stale replacement");
                    assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
                    let (cell_width, line_height) = view.cell_geom.map(|geom| (geom.cell_width, geom.line_height)).unwrap();
                    let geometry: Vec<_> = images.iter().map(|slice| {
                        assert_eq!(slice.placement.z, 0, "native images remain on their own layer");
                        slice.paint_bounds(point(px(0.), px(0.)), cell_width, line_height, 0, 0, 1.)
                    }).collect();
                    let bounds: Vec<_> = geometry.iter().map(|(crop, _)| *crop).collect();
                    let coverage: f32 = geometry.iter().map(|(crop, full)| {
                        crop.size.width.as_f32() * crop.size.height.as_f32()
                            / (full.size.width.as_f32() * full.size.height.as_f32())
                    }).sum();
                    assert!((coverage - 1.).abs() < 0.001, "complete raster coverage, not a partial placeholder/blank frame");
                    for (index, first) in bounds.iter().enumerate() {
                        for second in &bounds[index + 1..] {
                            assert!(first.origin.x + first.size.width <= second.origin.x
                                || second.origin.x + second.size.width <= first.origin.x
                                || first.origin.y + first.size.height <= second.origin.y
                                || second.origin.y + second.size.height <= first.origin.y);
                        }
                    }
                    assert!(bounds.iter().all(|b| b.origin.x >= px(0.) && b.origin.y >= px(0.)
                        && b.origin.x + b.size.width <= cell_width * 80.
                        && b.origin.y + b.size.height <= line_height * 24.));
                    let term = view.terminal.term.lock();
                    let row_of = |needle: &str| (0..24).find(|row| {
                        term.grid[Line(*row)][..].iter().map(|cell| cell.c()).collect::<String>().contains(needle)
                    }).unwrap_or_else(|| panic!("missing native text witness {needle}"));
                    let top = bounds.iter().map(|b| b.origin.y.as_f32() / line_height.as_f32()).fold(f32::INFINITY, f32::min);
                    let bottom = bounds.iter().map(|b| (b.origin.y + b.size.height).as_f32() / line_height.as_f32()).fold(0., f32::max);
                    assert!(top > row_of(&format!("NATIVE_STATIC_{phase}")) as f32);
                    assert!(bottom <= row_of("NATIVE_STATIC_AFTER") as f32);
                }
            });
            view.read_with(cx, |view, _| view.terminal.write(b"n".to_vec()));
            native_picture_wait(&view, cx, "native unmount clears visible placements", |view| {
                view.scrollback_text().contains("NATIVE_STATIC_EMPTY") && graphics_visible_images(view).is_empty()
            });
            graphics_assert_production_paint(window, &view, cx);
            view.read_with(cx, |view, _| view.terminal.write(b"n".to_vec()));
        }
        native_picture_wait(&view, cx, "native component exit audit and shell restoration", |view| {
            let text = view.scrollback_text();
            text.contains("NATIVE_STATIC_AUDITED") && text.contains("LYREL_PARENT_AFTER=lyrel")
        });
        assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
        graphics_recovery_destroy(view, window, cx);
    }
}

#[gpui_kit::test]
#[ignore = "isolated pinned Kimi source + native Node dependencies; no clipboard or model use"]
fn graphics_consumer_kimi_native_pasted_thumbnail_only(cx: &mut TestAppContext) {
    native_static_exercise(cx, crate::core::cli_agent::CLIAgent::Kimi);
}

#[gpui_kit::test]
#[ignore = "isolated Hermes v0.23.1 source, Python/Pillow and native TUI dependencies; image.render only"]
fn graphics_consumer_hermes_native_attachment_and_eligible_tool_images(cx: &mut TestAppContext) {
    native_static_exercise(cx, crate::core::cli_agent::CLIAgent::Hermes);
}
