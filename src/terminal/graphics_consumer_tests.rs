mod graphics_pty_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/kitty_pty.rs"
    ));
}

mod sync_capture_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/synchronized_capture.rs"
    ));
}

#[test]
#[ignore = "isolated daemon subprocess for the production GPUI consumer tests"]
fn graphics_fixture_daemon() {
    let Some(dir) = std::env::var_os("LYREL_GRAPHICS_TEST_DAEMON") else {
        return;
    };
    lyrel_core::core::config::set_config_dir(dir.into());
    lyrel_core::daemon::server::run_daemon().unwrap();
}

#[test]
#[ignore = "real PTY sender subprocess shared with server client tests"]
fn graphics_fixture_sender() {
    graphics_pty_fixture::pty_sender();
}

include!("graphics_media_consumer_tests.rs");

struct GraphicsDaemonProcess {
    child: std::process::Child,
    dir: tempfile::TempDir,
}

impl GraphicsDaemonProcess {
    fn start() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact"])
            .arg(format!(
                "{}::graphics_fixture_daemon",
                module_path!().split_once("::").unwrap().1
            ))
            .args(["--nocapture", "--test-threads=1"])
            // macOS production startup re-execs with its responsibility flag.
            // Keep that appended flag a harmless libtest filter, not an option.
            .arg("--")
            .env("LYREL_GRAPHICS_TEST_DAEMON", dir.path())
            .env("LYREL_CONFIG_DIR", dir.path())
            .env("LYREL_DATA_DIR", dir.path())
            .env("LYREL_CONTROL_SOCK", dir.path().join("control.sock"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self { child, dir };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let ready = (|| -> std::io::Result<()> {
                use lyrel_core::daemon::control::{
                    ControlClientMsg, ControlHello, ControlServerMsg,
                };

                // Bound the actual protocol reads/writes too: the outer startup
                // deadline alone cannot interrupt a silent listening handler.
                for endpoint in ["control", "daemon"] {
                    let mut stream =
                        crate::daemon::transport::connect_endpoint_at(&daemon.endpoint(endpoint))?;
                    let wait = deadline
                        .checked_duration_since(std::time::Instant::now())
                        .filter(|wait| !wait.is_zero())
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "graphics daemon startup deadline expired",
                            )
                        })?
                        .min(std::time::Duration::from_millis(250));
                    stream.set_read_timeout(Some(wait))?;
                    stream.set_write_timeout(Some(wait))?;
                    if endpoint == "control" {
                        let hello = ControlHello::host_rpc("graphics-gpui", "graphics-gpui");
                        ControlClientMsg::Hello(hello.clone()).encode(&mut stream)?;
                        match ControlServerMsg::read(&mut stream)? {
                            ControlServerMsg::HelloOk(ok)
                                if ok.control_version == hello.control_version => {}
                            other => {
                                return Err(std::io::Error::other(format!(
                                    "unexpected control readiness reply: {other:?}"
                                )));
                            }
                        }
                    } else {
                        ClientMsg::Version.encode(&mut stream)?;
                        if !matches!(DaemonMsg::read(&mut stream)?, DaemonMsg::Version(_)) {
                            return Err(std::io::Error::other("unexpected pane readiness reply"));
                        }
                    }
                }
                Ok(())
            })();
            if ready.is_ok() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "isolated production daemon did not become ready: {ready:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        daemon
    }

    fn endpoint(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(format!(
            "{name}.{}",
            if cfg!(windows) { "port" } else { "sock" }
        ))
    }

    fn panes(&self) -> lyrel_core::client::PaneClient {
        lyrel_core::client::PaneClient::at(self.endpoint("daemon"))
    }

    fn spawn(&self) -> u64 {
        use crate::daemon::protocol::{ShellSpec, WinSize};
        let shell = ShellSpec {
            program: std::env::current_exe().unwrap().to_str().unwrap().into(),
            args: vec![
                "--ignored".into(),
                "--exact".into(),
                format!(
                    "{}::graphics_fixture_sender",
                    module_path!().split_once("::").unwrap().1
                ),
                "--nocapture".into(),
                "--color=never".into(),
                "--test-threads=1".into(),
            ],
            args_are_lyrel_defaults: false,
        };
        let mut session = self
            .panes()
            .spawn(
                None,
                WinSize {
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
        session
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let mut text = Vec::new();
        while !text
            .windows(b"KITTY_READY".len())
            .any(|part| part == b"KITTY_READY")
        {
            // The child may print before its first subscriber attaches.
            match session.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => text.extend(bytes),
                DaemonMsg::Error(error) => panic!("graphics sender failed: {error}"),
                DaemonMsg::Exited { code } => {
                    panic!("graphics sender exited before KITTY_READY: {code:?}")
                }
                _ => {}
            }
        }
        let id = session.pane_id();
        session.detach().unwrap();
        id
    }

    fn stream(&self, pane: u64) -> Stream {
        self.stream_with_capacity(pane, 64 << 20, 128 << 20)
    }

    fn stream_with_capacity(&self, pane: u64, encoded_bytes: u64, decoded_bytes: u64) -> Stream {
        self.stream_with_capacity_at(
            pane,
            encoded_bytes,
            decoded_bytes,
            crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
        )
    }

    fn stream_with_capacity_at(
        &self,
        pane: u64,
        encoded_bytes: u64,
        decoded_bytes: u64,
        size: crate::daemon::protocol::WinSize,
    ) -> Stream {
        let mut stream =
            crate::daemon::transport::connect_endpoint_at(&self.endpoint("daemon")).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
        ClientMsg::GraphicsCapacity {
            encoded_bytes,
            decoded_bytes,
            offers: true,
        }
        .encode(&mut stream)
        .unwrap();
        stream.set_read_timeout(None).unwrap();
        ClientMsg::Attach {
            pane_id: pane,
            size,
            allow_remote_clipboard_write: false,
        }
        .encode(&mut stream)
        .unwrap();
        stream
    }
}

impl Drop for GraphicsDaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn graphics_visible_images(view: &TerminalView) -> Vec<crate::terminal::images::RenderPlacement> {
    use lyrel_core::core::kitty_graphics::placeholder_row;
    let term = view.terminal.term.lock();
    let offset = term.display_offset() as i32;
    let rows: Vec<_> = (0..term.screen_lines())
        .filter_map(|row| {
            let slices = placeholder_row(&term.grid, Line(row as i32 - offset));
            (!slices.is_empty()).then_some((row as i32, slices))
        })
        .collect();
    view.terminal.images().snapshot_with_placeholders(&rows)
}

fn graphics_send_fixture(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
) {
    graphics_send_fixture_with_replies(view, cx, serial, bytes, 0);
}

fn graphics_send_fixture_with_replies(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
    replies: usize,
) {
    let operation = if replies == 0 { "SEND" } else { "SEND_OK" };
    let request = format!(
        "{operation} {serial} {replies} {} 0\n",
        graphics_pty_fixture::hex(bytes)
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let marker = format!("KITTY_DONE_{serial};");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // The visible marker can arrive before its trailing cursor restore,
        // especially through ConPTY. The title follows that restore in-stream.
        if view.read_with(cx, |view, _| {
            view.title == marker && view.scrollback_text().contains(&marker)
        }) {
            break;
        }
        assert!(
            !view.read_with(cx, |view, _| view.terminal.child_exited()),
            "PTY sender exited before serial {serial}; Input bytes={input_bytes}; output={}",
            view.read_with(cx, |view, _| view.scrollback_text())
        );
        assert!(
            std::time::Instant::now() < deadline,
            "PTY round trip did not finish: {serial}; Input bytes={input_bytes}; child_exited={}; disconnected={}; tail={:?}",
            view.read_with(cx, |view, _| view.terminal.child_exited()),
            view.read_with(cx, |view, _| view.terminal.exited),
            view.read_with(cx, |view, _| view
                .scrollback_text()
                .chars()
                .rev()
                .take(256)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>())
        );
        graphics_consumer_progress(cx);
    }
}

fn graphics_rooted_consumer(
    cx: &mut TestAppContext,
    pane: u64,
    stream: Stream,
) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
    let (view, window) = graphics_build_rooted_consumer(cx, pane, stream, Vec::new());
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(80, 24));
    graphics_send_fixture(&view, cx, 0, b"\x1b[0m");
    (view, window)
}

fn graphics_build_rooted_consumer(
    cx: &mut TestAppContext,
    pane: u64,
    stream: Stream,
    buffered: Vec<u8>,
) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
    let source = if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    };
    graphics_build_rooted_consumer_at(cx, pane, stream, buffered, TermSize::new(80, 24), source)
}

fn graphics_build_rooted_consumer_at(
    cx: &mut TestAppContext,
    pane: u64,
    stream: Stream,
    buffered: Vec<u8>,
    size: TermSize,
    source: PtySource,
) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
    // App::open_window draws its root before returning. Mounting the terminal
    // in a default maximized window would already resize the living pane once
    // before the requested 80/40-column physical geometry can be installed.
    let window: gpui_kit::AnyWindowHandle = cx.add_window(|_, _| gpui_kit::EmptyView).into();
    let view = window
        .update(cx, |_, window, cx| {
            let terminal =
                RemoteTerminal::from_stream_with(stream, size, buffered, source).unwrap();
            cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx))
        })
        .unwrap();
    let (target, _, _, _) = graphics_window_geometry(window, &view, cx, size);
    cx.simulate_window_resize(window, target);
    window
        .update(cx, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                gpui_kit::component::Root::new(view.clone(), window, cx)
            });
        })
        .unwrap();
    (view, window)
}

#[gpui_kit::test]
fn graphics_consumer_synchronized_output_keeps_stream_ordered_image_anchors(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    graphics_send_fixture(
        &view,
        cx,
        1,
        concat!(
            "\x1b[?1049h\x1b[2J\x1b[H\x1b[?2026h",
            "\x1b[4;3H\x1b_Ga=T,i=711,p=1,c=4,r=2,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
            "\x1b[8;12H\x1b_Ga=T,i=712,p=1,c=4,r=2,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
            "\x1b[?2026l"
        )
        .as_bytes(),
    );
    graphics_recovery_wait(cx, "synchronized pictures decoded", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 2 && images.iter().all(|image| image.data.is_some())
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        for (id, col, row, bgra) in [
            (711, 2, 3, [0, 0, 255, 255]),
            (712, 11, 7, [0, 255, 0, 255]),
        ] {
            let image = images
                .iter()
                .find(|image| image.placement.image_id == id)
                .unwrap();
            assert_eq!(
                (image.anchor_col, image.anchor_row),
                (col, row),
                "image {id} uses its in-stream cursor, not the previous synchronized batch"
            );
            assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), bgra);
            assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
        }
    });
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn graphics_consumer_real_pty_daemon_unicode_pixels_follow_cells_and_share_direct_resource(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let stream = daemon.stream(pane);
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, stream);
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[3J\x1b_Ga=T,i=16777223,p=9,U=1,c=4,r=1,f=32,s=4,v=1,q=2;/wAA/wD/AP8AAP///////w==\x1b\\");
    assert!(
        view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "prototype cannot paint on its own"
    );
    graphics_assert_production_paint(window.into(), &view, cx);
    // High byte=1, indexed low byte=7, explicit placement=9. Subsequent
    // cells exercise zero-, one- and two-diacritic inheritance independently.
    graphics_send_fixture(&view, cx, 2, "\x1b[6;5H\x1b[38;5;7;58;5;9m\u{10eeee}\u{305}\u{305}\u{30d}\u{10eeee}\u{10eeee}\u{305}\u{10eeee}\u{305}\u{310}\x1b[0m".as_bytes());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| !graphics_visible_images(view).is_empty()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "production decoder did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    graphics_assert_production_paint(window.into(), &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].placement.image_id, 16777223);
        assert_eq!(images[0].placement.placement_id, 9);
        // Sender RGBA pixels are red, green, blue, opaque white (4 × 4 bytes).
        assert_eq!(images[0].data.as_ref().unwrap().as_bytes(0).unwrap(), &[
            0, 0, 255, 255,
            0, 255, 0, 255,
            255, 0, 0, 255,
            255, 255, 255, 255,
        ]);
        let (crop, full) = images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(crop, Bounds::new(point(px(40.), px(50.)), gpui_kit::size(px(40.), px(10.))));
        assert_eq!(crop, full);
        assert!(images[0].painted.load(Ordering::Acquire), "actual TerminalElement paint claimed the production-decoded texture; not final GPU pixels");
    });
    // Direct display shares the same image, while physical selectors leave
    // the text-derived image untouched. Deleting one cell clips its source.
    graphics_send_fixture(
        &view,
        cx,
        3,
        b"\x1b[10;2H\x1b_Ga=p,i=16777223,p=10,c=4,r=1,C=1,q=2\x1b\\",
    );
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 2);
        assert!(std::sync::Arc::ptr_eq(
            images[0].data.as_ref().unwrap(),
            images[1].data.as_ref().unwrap()
        ));
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 4, b"\x1b_Ga=d,d=A\x1b\\\x1b[6;6H\x1b[X");
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        let (crop, full) =
            images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        // Erasing the second cell breaks high-byte inheritance to its right;
        // only the explicit first source cell remains a valid reference.
        assert_eq!(crop.size.width, px(10.));
        assert_eq!(full.size.width, px(40.));
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 5, b"\x1b[6;1H\x1b[2K");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_assert_production_paint(window.into(), &view, cx);
    // True-color identity, omitted placement selection and missing references.
    graphics_send_fixture(&view, cx, 6, "\x1b_Ga=T,i=66051,p=2,U=1,c=4,r=1,f=32,s=4,v=1,q=2;/wAA/wD/AP8AAP///////w==\x1b\\\x1b[7;4H\x1b[38;2;1;2;3m\u{10eeee}\u{305}\u{30d}\u{10eeee}\x1b[38;2;9;9;9m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| graphics_visible_images(view).len() == 1) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images[0].placement.image_id, 66051);
        let (crop, full) =
            images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(
            crop,
            Bounds::new(point(px(30.), px(60.)), gpui_kit::size(px(20.), px(10.)))
        );
        assert_eq!(full.origin, point(px(20.), px(60.)));
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 7, b"\x1b[7;1H\x1b[L");
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let (crop, _) = images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px(70.),
            "IL moves actual placeholder text and its image together"
        );
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 8, b"\x1b[?1049h");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 9, b"\x1b[?1049l");
    assert_eq!(
        view.read_with(cx, |view, _| graphics_visible_images(view).len()),
        1
    );
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 10, b"\x1b_Ga=d,d=I,i=66051\x1b\\");
    assert!(
        view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "deleted prototype cannot revive from unchanged PUA text"
    );
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 11, "\x1b[2J\x1b_Ga=T,i=7,p=9,U=1,c=4,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[6;5H\x1b[38;5;7;58;5;9m\u{10eeee}\u{305}\u{305}\u{30d}\u{10eeee}\u{305}\u{30e}\u{10eeee}\u{301}\x1b[0m".as_bytes());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| graphics_visible_images(view).len() == 2) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let ids: std::collections::BTreeSet<_> = images
            .iter()
            .map(|image| image.placement.image_id)
            .collect();
        assert_eq!(
            ids,
            [7, 16777223].into_iter().collect(),
            "nonconsecutive explicit column resets high-byte inheritance"
        );
        assert!(
            images
                .iter()
                .any(|image| image.data.as_ref().unwrap().as_bytes(0).unwrap() == [255, 0, 0, 255])
        );
        assert_eq!(
            images.len(),
            2,
            "invalid combining mark must not alias row/column zero"
        );
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 12, b"\x1b[5;9r\x1b[9;1H\n");
    view.read_with(cx, |view, _| {
        for image in graphics_visible_images(view) {
            let (crop, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
            assert_eq!(
                crop.origin.y,
                px(40.),
                "regional scroll uses underlying cell movement"
            );
        }
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 13, b"\x1b[r\x1b[2J\x1b[3J");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 14, "\x1b[6;5H\x1b[38;5;7;58;5;9m\u{10eeee}\u{305}\u{305}\u{30d}\u{10eeee}\u{10eeee}\u{10eeee}\x1b[0m".as_bytes());
    graphics_send_fixture(&view, cx, 15, &b"\r\n".repeat(100));
    view.update(cx, |view, _| {
        view.terminal.term.lock().scroll_display(Scroll::Top)
    });
    assert!(
        !view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "scrolling into retained history restores the same placeholder cells"
    );
    graphics_assert_production_paint(window.into(), &view, cx);
    view.update(cx, |view, _| {
        view.terminal.term.lock().scroll_display(Scroll::Bottom)
    });
    graphics_send_fixture(&view, cx, 16, &b"\r\n".repeat(10_100));
    view.update(cx, |view, _| {
        view.terminal.term.lock().scroll_display(Scroll::Top)
    });
    assert!(
        view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "truncated cells cannot drift back over newer output"
    );
    graphics_assert_production_paint(window.into(), &view, cx);
    view.update(cx, |view, _| {
        view.terminal.term.lock().scroll_display(Scroll::Bottom)
    });
    graphics_send_fixture(&view, cx, 17, "\x1b[2J\x1b[3J\x1b_Ga=T,i=42,p=6,U=1,c=2,r=2,f=32,s=2,v=2,q=2;/wAA/wD/AP8AAP///////w==\x1b\\\x1b[6;5H\x1b[38;5;42;58;5;6m\u{10eeee}\u{30d}\u{305}\u{10eeee}\x1b[58;5;99m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| graphics_visible_images(view).len() == 1) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(
            images.len(),
            1,
            "nonzero missing placement identity cannot fall back to another prototype"
        );
        let (crop, full) =
            images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(
            crop,
            Bounds::new(point(px(40.), px(50.)), gpui_kit::size(px(20.), px(10.)))
        );
        assert_eq!(
            full,
            Bounds::new(point(px(40.), px(40.)), gpui_kit::size(px(20.), px(20.))),
            "row diacritic selects the lower half of the real texture"
        );
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(
        &view,
        cx,
        18,
        "\x1b[2J\x1b[3J\x1b[6;50H\x1b[38;5;42;58;5;6m\u{10eeee}\u{30d}\u{305}\u{10eeee}\x1b[0m"
            .as_bytes(),
    );
    // Resize through the rooted window so prepaint owns both the grid and cell
    // metrics. A second, synthetic cell resize would be undone by the next
    // frame and could cancel the sender's in-flight Kitty query barrier.
    graphics_assert_production_paint_at(window.into(), &view, cx, TermSize::new(40, 24));
    graphics_send_fixture(&view, cx, 19, b"\x1b[0m");
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        let (crop, full) =
            images[0].paint_bounds(point(px(0.), px(0.)), px(15.), px(15.), 0, 0, 1.5);
        assert_eq!(
            crop.origin.x,
            px(135.),
            "Rio reflow moves column 49 to column 9, not the prototype anchor"
        );
        assert_eq!(crop.size, gpui_kit::size(px(30.), px(15.)));
        assert_eq!(
            full.size,
            gpui_kit::size(px(30.), px(30.)),
            "new logical cell geometry scales the same retained texture"
        );
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    let prior = view.read_with(cx, |view, _| graphics_visible_images(view).remove(0));
    graphics_send_fixture(&view, cx, 20, "\x1b[2J\x1b[3J\x1b_Ga=p,i=42,p=66051,U=1,c=2,r=2,q=2\x1b\\\x1b[6;5H\x1b[38;5;42;58;2;1;2;3m\u{10eeee}\u{30d}\u{305}\u{10eeee}\x1b[0m".as_bytes());
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0].placement.placement_id, 66051,
            "true-color underline selects the exact prototype, not the lowest id"
        );
        assert_eq!(images[0].placement.resource, prior.placement.resource);
        assert!(
            std::sync::Arc::ptr_eq(
                images[0].data.as_ref().unwrap(),
                prior.data.as_ref().unwrap()
            ),
            "multiple prototypes do not decode or copy their shared resource again"
        );
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_budget_notices_share_direct_and_placeholder_history(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    // Two RGBA pixels fit. The third must reclaim history, not the live image.
    let stream = daemon.stream_with_capacity(pane, 1024, 8);
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, stream);
    graphics_send_fixture(&view, cx, 1, "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b_Ga=T,i=8,p=2,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[8;5H\x1b[38;5;8;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while view.read_with(cx, |view, _| graphics_visible_images(view).len()) != 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "both production placement paths must decode"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    graphics_send_fixture(&view, cx, 2, &b"\r\n".repeat(80));
    graphics_send_fixture(
        &view,
        cx,
        3,
        b"\x1b[10;5H\x1b_Ga=T,i=9,p=3,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
    );
    view.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    window.update(cx, |_, window, _| window.refresh()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let old = images
            .iter()
            .find(|image| image.placement.image_id == 7)
            .unwrap();
        assert!(
            old.evicted && old.data.is_none(),
            "oldest direct history becomes pixel-free metadata"
        );
        assert!(
            old.painted.load(std::sync::atomic::Ordering::Acquire),
            "the real TerminalElement claims the localized notice in the retained area"
        );
        let (area, _) = old.paint_bounds(
            point(px(0.), px(0.)),
            px(10.),
            px(10.),
            0,
            view.terminal.term.lock().display_offset() as i32,
            1.,
        );
        assert!(
            area.size.width > px(0.) && area.size.height > px(0.),
            "notice reuses the original image area"
        );
        assert!(
            !images
                .iter()
                .find(|image| image.placement.image_id == 8)
                .unwrap()
                .evicted,
            "newer placeholder history remains resident first"
        );
        assert!(
            !view.scrollback_text().contains("Image evicted")
                && !view.scrollback_text().contains("图片已淘汰"),
            "notice is paint-only, never terminal text"
        );
    });
    view.update(cx, |view, cx| {
        view.scroll(-10_000, None, &Modifiers::default(), cx)
    });
    graphics_send_fixture(
        &view,
        cx,
        4,
        b"\x1b[12;5H\x1b_Ga=T,i=10,p=4,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/////w==\x1b\\",
    );
    view.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let placeholder = images
            .iter()
            .find(|image| image.placement.image_id == 8)
            .unwrap();
        assert!(
            placeholder.evicted && placeholder.data.is_none(),
            "the same retention rule projects a notice through real placeholder cells"
        );
        assert!(
            placeholder
                .painted
                .load(std::sync::atomic::Ordering::Acquire),
            "the real placeholder cells must paint the localized notice"
        );
    });
    view.update(cx, |view, cx| {
        view.scroll(-10_000, None, &Modifiers::default(), cx)
    });
    // Both retained pixels are now live; refused data cannot manufacture a
    // placement or replace those pixels, even when success/error replies mute.
    let cursor = view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0)
    });
    graphics_send_fixture(
        &view,
        cx,
        5,
        b"\x1b_Ga=T,i=11,p=5,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
    );
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert!(!images.iter().any(|image| image.placement.image_id == 11));
        let term = view.terminal.term.lock();
        assert_eq!(
            (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0),
            cursor,
            "refusal does not apply the default placement cursor movement"
        );
    });
    graphics_send_fixture(&view, cx, 6, b"shell_still_usable");
    assert!(view.read_with(cx, |view, _| {
        view.scrollback_text().contains("shell_still_usable")
    }));
    graphics_send_fixture(
        &view,
        cx,
        7,
        b"\x1b_Ga=d,d=I,i=7\x1b\\\x1b_Ga=d,d=I,i=8\x1b\\",
    );
    view.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    assert!(
        !view.read_with(cx, |view, _| graphics_visible_images(view)
            .iter()
            .any(|image| [7, 8].contains(&image.placement.image_id))),
        "intentional deletion removes both notices without changing placeholder text"
    );
    view.update(cx, |view, cx| {
        view.scroll(-10_000, None, &Modifiers::default(), cx)
    });
    graphics_send_fixture(&view, cx, 8, b"\x1b[2J\x1b[3J");
    assert!(
        view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "clear leaves no replacement notice"
    );
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_cross_daemon_aggregate_rejects_without_losing_pixels_then_releases_on_close(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    // Real atlas pages round a one-pixel image up to 1024² BGRA pixels.
    const GPU_PAGE: usize = 4 << 20;
    crate::terminal::images::with_test_cache_limits(
        2 * GPU_PAGE + 1024,
        2 * GPU_PAGE + 1024,
        || {
            let daemons: Vec<_> = (0..3).map(|_| GraphicsDaemonProcess::start()).collect();
            cx.update(|cx| {
                gpui_kit::component::init(cx);
                cx.set_global(Config::default());
            });
            let mut terminals = Vec::new();
            for serial in 0..3 {
                let daemon = &daemons[serial];
                let pane = daemon.spawn();
                // Replacement admission holds old and new decoded frames at once;
                // desktop aggregate pressure, not an undersized daemon cap, is under test.
                let stream = daemon.stream_with_capacity(pane, 1024, 8);
                let (view, window) = graphics_rooted_consumer(cx, pane, stream);
                graphics_send_fixture(
                    &view,
                    cx,
                    serial + 1,
                    b"\x1b[5;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
                );
                if serial < 2 {
                    graphics_layout_wait_image(&view, cx, 7);
                }
                terminals.push((window, view, pane, serial));
            }
            assert!(
                terminals[2]
                    .1
                    .read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
                "aggregate admission rejects a third resource before accepting pixels, not an unfinishable decode queue"
            );
            for (_, view, _, _) in &terminals[..2] {
                view.read_with(cx, |view, _| {
                    let images = graphics_visible_images(view);
                    assert_eq!(
                        images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                        &[0, 0, 255, 255]
                    );
                    assert!(
                        !images[0].evicted,
                        "cache pressure cannot replace a visible accepted image with a notice"
                    );
                });
            }
            // Only the oldest pane moves into history. It belongs to a different
            // daemon from the new offer, but both compete for the desktop pool.
            graphics_send_fixture(&terminals[0].1, cx, 4, &b"\r\n".repeat(80));
            terminals[0].1.update(cx, |view, cx| {
                view.scroll(10_000, None, &Modifiers::default(), cx)
            });
            {
                // Confirm the history owner's viewport before another daemon
                // attempts pressure; the two pane links have no shared ordering.
                graphics_send_fixture(&terminals[0].1, cx, 39, b"\x1b[0m");
                graphics_send_fixture(
                    &terminals[2].1,
                    cx,
                    40,
                    b"\x1b[5;5H\x1b_Ga=T,i=9,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
                );
            };
            assert!(
                terminals[2]
                    .1
                    .read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
                "another daemon cannot reclaim history the user is actually viewing"
            );
            terminals[0].1.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert!(!images[0].evicted);
                assert_eq!(
                    images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                    [0, 0, 255, 255]
                );
            });
            terminals[0].1.update(cx, |view, cx| {
                view.scroll(-10_000, None, &Modifiers::default(), cx)
            });
            {
                // The viewport and pressure travel to different daemons. Complete
                // a real round trip on the history owner before the new offer.
                graphics_send_fixture(&terminals[0].1, cx, 41, b"\x1b[0m");
                let request = format!(
                    "SEND_OK 5 1 {} 0\n",
                    graphics_pty_fixture::hex(
                        b"\x1b[5;5H\x1b_Ga=T,i=8,p=1,c=2,r=1,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\"
                    )
                );
                terminals[2]
                    .1
                    .read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
                // Pump the old window's atlas retirement while the offering
                // daemon is still inside its bounded admission wait, not after
                // KITTY_DONE_5 has already certified a possibly refused upload.
                graphics_recovery_wait(cx, "PTY round trip did not finish: 5", |cx| {
                    for index in [0, 2] {
                        terminals[index]
                            .0
                            .update(cx, |_, window, _| window.refresh())
                            .unwrap();
                    }
                    terminals[2].1.read_with(cx, |view, _| {
                        assert!(
                            !view.terminal.child_exited(),
                            "PTY sender exited before serial 5; output={}",
                            view.scrollback_text()
                        );
                        view.scrollback_text().contains("KITTY_DONE_5;")
                    })
                });
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while terminals[2]
                .1
                .read_with(cx, |view, _| graphics_visible_images(view).is_empty())
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "cross-source pressure must reclaim eligible history before refusing usable capacity"
                );
                for index in [0, 2] {
                    terminals[index]
                        .0
                        .update(cx, |_, window, _| window.refresh())
                        .unwrap();
                }
                cx.executor()
                    .advance_clock(std::time::Duration::from_millis(5));
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            terminals[0].1.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert!(
                    images[0].evicted && images[0].data.is_none(),
                    "the other daemon must authoritatively announce historical eviction"
                );
            });
            graphics_send_fixture(
                &terminals[2].1,
                cx,
                6,
                b"\x1b_Ga=T,i=8,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
            );
            terminals[2].1.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert_eq!(
                    images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[0, 0, 255, 255],
                    "refused replacement keeps the current red resource intact"
                );
            });
            let (closed_window, closed_view, closed_pane, source) = terminals.remove(1);
            daemons[source].panes().kill(closed_pane).unwrap();
            drop(closed_view);
            closed_window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
            cx.run_until_parked();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut serial = 7;
            loop {
                cx.executor()
                    .advance_clock(std::time::Duration::from_millis(350));
                cx.run_until_parked();
                graphics_send_fixture(
                    &terminals[1].1,
                    cx,
                    serial,
                    b"\x1b[5;5H\x1b_Ga=T,i=8,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
                );
                serial += 1;
                if terminals[1].1.read_with(cx, |view, _| {
                    graphics_visible_images(view).iter().any(|image| {
                        image
                            .data
                            .as_ref()
                            .is_some_and(|data| data.as_bytes(0).unwrap() == [0, 255, 0, 255])
                    })
                }) {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let (diagnostics, text, allocations) =
                        terminals[1].1.read_with(cx, |view, _| {
                            (
                                view.terminal.images().recovery_diagnostics(),
                                view.scrollback_text(),
                                view.terminal.images().allocation_observation(),
                            )
                        });
                    panic!(
                        "closing a pane must release actual residency: diagnostics={diagnostics}; allocations={allocations}; text={text}"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            terminals[1].1.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert_eq!(
                    images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[0, 255, 0, 255]
                );
            });
            for (_, _, pane, source) in terminals {
                daemons[source].panes().kill(pane).unwrap();
            }
        },
    );
}

#[gpui_kit::test]
fn graphics_consumer_five_idle_panes_do_not_starve_three_visible_small_images(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    crate::terminal::images::with_test_cache_limits(384 << 20, 1024 << 20, || {
        let daemon = GraphicsDaemonProcess::start();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
        });
        let mut terminals = Vec::new();
        for _ in 0..5 {
            let pane = daemon.spawn();
            let stream = daemon.stream(pane);
            let (view, window) = graphics_rooted_consumer(cx, pane, stream);
            terminals.push((window, view, pane));
        }
        // All five live consumers exist before any source is offered. An idle
        // pane must not permanently reserve a full pane's source allowance.
        graphics_send_fixture(&terminals[4].1, cx, 1, "\x1b[2J\x1b[3J\x1b[5;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[7;5H\x1b_Ga=T,i=8,p=2,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\\x1b_Ga=T,i=9,p=3,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[9;5H\x1b[38;5;9;58;5;3m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while terminals[4]
            .1
            .read_with(cx, |view, _| graphics_visible_images(view).len())
            != 3
        {
            assert!(
                std::time::Instant::now() < deadline,
                "a fifth pane must paint its first output and more than two visible small resources"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        terminals[4]
            .0
            .update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
        terminals[4].1.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            assert!(images.iter().all(|image| !image.evicted && image.data.is_some()));
            assert!(images.iter().all(|image| image.painted.load(std::sync::atomic::Ordering::Acquire)), "production TerminalElement must claim all three actual textures, not just receive resources");
            let pixels: Vec<_> = images.iter().map(|image| image.data.as_ref().unwrap().as_bytes(0).unwrap().to_vec()).collect();
            assert_eq!(pixels, [vec![0, 0, 255, 255], vec![0, 255, 0, 255], vec![255, 0, 0, 255]]);
        });
        for (serial, (_, view, _)) in terminals[..4].iter().enumerate() {
            graphics_send_fixture(
                view,
                cx,
                serial + 2,
                b"\x1b[5;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "every previously idle pane can receive a small image within actual byte budgets"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        for (_, _, pane) in terminals {
            daemon.panes().kill(pane).unwrap();
        }
    });
}

#[gpui_kit::test]
fn graphics_consumer_metadata_grants_bound_places_and_live_snapshots_without_pixel_eviction(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    crate::terminal::images::with_test_metadata_limits(1024, 1024, || {
        let daemon = GraphicsDaemonProcess::start();
        let pane = daemon.spawn();
        let stream = daemon.stream(pane);
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
        });
        let (view, _window) = graphics_rooted_consumer(cx, pane, stream);
        graphics_send_fixture(
            &view,
            cx,
            1,
            b"\x1b[5;5H\x1b_Ga=t,i=7,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
        );
        assert!(
            view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
            "one resource owner consumes metadata without inventing a placement"
        );
        graphics_send_fixture(&view, cx, 2, b"\x1b_Ga=p,i=7,p=1,c=2,r=1,C=1,q=2\x1b\\");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let retained_snapshot =
            view.read_with(cx, |view, _| graphics_visible_images(view).remove(0));
        graphics_recovery_wait(
            cx,
            "initial placement cursor restore was not consumed",
            |cx| {
                view.read_with(cx, |view, _| {
                    let term = view.terminal.term.lock();
                    (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0) == (4, 4)
                })
            },
        );
        let cursor = view.read_with(cx, |view, _| {
            let term = view.terminal.term.lock();
            (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0)
        });
        graphics_send_fixture(&view, cx, 3, b"\x1b_Ga=p,i=7,p=2,c=2,r=1,q=2\x1b\\\x1b_Ga=T,i=8,p=3,c=2,r=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
        graphics_recovery_wait(cx, "refusal cursor restore was not consumed", |cx| {
            view.read_with(cx, |view, _| {
                let term = view.terminal.term.lock();
                (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0) == cursor
            })
        });
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            assert_eq!(
                images.len(),
                1,
                "metadata-only placement and new transmit-and-display are both refused atomically"
            );
            assert_eq!(images[0].placement.placement_id, 1);
            assert_eq!(
                images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                &[0, 0, 255, 255]
            );
            assert!(
                !images[0].evicted,
                "metadata shortage cannot create a useless pixel eviction notice"
            );
            let term = view.terminal.term.lock();
            assert_eq!(
                (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0),
                cursor
            );
        });
        graphics_send_fixture(
            &view,
            cx,
            4,
            b"\x1b_Ga=d,d=i,i=7,p=1\x1b\\\x1b_Ga=p,i=7,p=2,c=2,r=1,C=1,q=2\x1b\\",
        );
        assert!(
            view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
            "a removed placement's live snapshot retains its metadata credit until actual drop"
        );
        drop(retained_snapshot);
        graphics_send_fixture(&view, cx, 5, b"\x1b_Ga=p,i=7,p=2,c=2,r=1,C=1,q=2\x1b\\");
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].placement.placement_id, 2);
            assert_eq!(
                images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                &[0, 0, 255, 255]
            );
        });
        graphics_send_fixture(
            &view,
            cx,
            6,
            b"\x1b_Ga=d,d=I,i=7\x1b\\\x1b_Ga=T,i=8,p=3,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()) {
            assert!(
                std::time::Instant::now() < deadline,
                "explicit resource deletion releases both metadata owners"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        graphics_send_fixture(&view, cx, 7, b"\x1bc");
        assert!(
            view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
            "reset removes metadata without notices"
        );
        daemon.panes().kill(pane).unwrap();
    });
}

fn graphics_retained_pane_clear(cx: &mut TestAppContext, child_exit: bool, history: bool) {
    use std::sync::atomic::Ordering;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let stream = daemon.stream(pane);
    let disconnect = stream.try_clone().unwrap();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, stream);
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[3J\x1b[6;1HHISTORY-TEXT\x1b_Ga=T,i=101,f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\");
    graphics_layout_wait_image(&view, cx, 101);
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(&view, cx, 2, &b"\r\n".repeat(60));
    graphics_send_fixture(
        &view,
        cx,
        3,
        b"\x1b[6;1HSCREEN-TEXT\x1b_Ga=T,i=102,f=32,s=1,v=1,c=1,r=1,C=1,q=2;AAD//w==\x1b\\",
    );
    graphics_layout_wait_image(&view, cx, 102);
    graphics_assert_production_paint(window, &view, cx);
    let images = view.read_with(cx, |view, _| {
        assert!(view.scrollback_text().contains("HISTORY-TEXT"));
        assert!(view.scrollback_text().contains("SCREEN-TEXT"));
        assert!(view.terminal.term.lock().grid.history_size() > 0);
        let images = view.terminal.images().snapshot();
        assert!(
            images
                .iter()
                .all(|image| image.painted.load(Ordering::Acquire)),
            "both textures must have reached production TerminalElement paint"
        );
        images
    });
    if child_exit {
        view.read_with(cx, |view, _| {
            view.terminal.write(b"EXIT 4 0 00 0\n".to_vec())
        });
    } else {
        disconnect.shutdown(std::net::Shutdown::Both).unwrap();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.update(cx, |view, _| {
            view.terminal.poll_exited();
            view.terminal.exited
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "production reader did not observe exit/EOF"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.update(cx, |view, cx| {
        assert_eq!(view.terminal.child_exited(), child_exit);
        let store = view.terminal.images();
        let prior_history = view.terminal.term.lock().grid.history_size();
        view.terminal.term.lock().selection = Some(Selection::new(
            SelectionType::Simple,
            Pos::new(Line(-1), Column(0)),
            Side::Left,
        ));
        if history {
            view.clear_scrollback(cx);
        } else {
            view.clear_screen(cx);
        }
        let term = view.terminal.term.lock();
        assert!(term.selection.is_none());
        assert!(
            term.bounds_to_string(
                Pos::new(Line(0), Column(0)),
                Pos::new(term.bottommost_line(), term.last_column())
            )
            .trim()
            .is_empty()
        );
        if history {
            assert_eq!(term.grid.history_size(), 0);
        } else {
            assert!(term.grid.history_size() >= prior_history);
        }
        drop(term);
        assert_eq!(view.scrollback_text().contains("HISTORY-TEXT"), !history);
        assert_eq!(
            store
                .snapshot()
                .iter()
                .map(|image| image.placement.image_id)
                .collect::<Vec<_>>(),
            if history { vec![] } else { vec![101] }
        );
        for image in &images {
            if history || image.placement.image_id == 102 {
                assert!(
                    !store.claim_for_paint(image),
                    "cleared placements cannot claim cached paint snapshots"
                );
            }
        }
    });
    window
        .update(cx, |_, window, cx| {
            let store = view.read(cx).terminal.images();
            store.release_retired(window, cx);
        })
        .unwrap();
    graphics_assert_production_paint(window, &view, cx);
    if !history {
        view.update(cx, |view, cx| {
            view.scroll(10_000, None, &Modifiers::default(), cx)
        });
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            let retained = graphics_visible_images(view)
                .into_iter()
                .find(|image| image.placement.image_id == 101)
                .expect("the retained historical placement must remain visible on scrollback");
            assert!(
                retained.painted.load(Ordering::Acquire),
                "local clear must not permanently fence the retained historical texture"
            );
        });
        view.update(cx, |view, cx| {
            view.scroll(-10_000, None, &Modifiers::default(), cx)
        });
    }
    if !child_exit {
        let mut session = daemon
            .panes()
            .attach(
                pane,
                crate::daemon::protocol::WinSize {
                    cols: 80,
                    rows: 24,
                    cell_w: 10,
                    cell_h: 20,
                },
            )
            .unwrap();
        session
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let mut replay = Vec::new();
        while !replay
            .windows(b"SCREEN-TEXT".len())
            .any(|part| part == b"SCREEN-TEXT")
        {
            match session.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => replay.extend(bytes),
                DaemonMsg::Exited { .. } => panic!("disconnect must not terminate the real child"),
                _ => {}
            }
        }
        session.detach().unwrap();
        daemon.panes().kill(pane).unwrap();
    }
}

#[gpui_kit::test]
fn graphics_consumer_user_clear_screen_works_after_real_exit_and_disconnect(
    cx: &mut TestAppContext,
) {
    for child_exit in [true, false] {
        graphics_retained_pane_clear(cx, child_exit, false);
    }
}

#[gpui_kit::test]
fn graphics_consumer_user_clear_scrollback_works_after_real_exit_and_disconnect(
    cx: &mut TestAppContext,
) {
    for child_exit in [true, false] {
        graphics_retained_pane_clear(cx, child_exit, true);
    }
}

#[gpui_kit::test]
fn graphics_consumer_large_text_replay_then_large_pixels_keeps_admitted_source(
    cx: &mut TestAppContext,
) {
    use base64::Engine as _;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (prior, prior_window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    // Each hex-encoded request stays below the production 1 MiB Input limit;
    // the unchanged aggregate text still becomes one large replay Snapshot.
    for serial in 1..=4 {
        graphics_send_fixture(&prior, cx, serial, &vec![b'X'; 192 << 10]);
    }
    prior.update(cx, |view, _| view.terminal.detach_link());
    prior_window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    drop(prior);
    // The real replay ring coalesces the large text into a Snapshot. The
    // production reader, not a synthetic allocation assertion, consumes it.
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let pixels = [255, 0, 0, 255].repeat(192 * 128);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&pixels);
    let command = format!(
        "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=3001,c=2,r=1,C=1,f=32,s=192,v=128;{encoded}\x1b\\"
    );
    graphics_send_fixture_with_replies(&view, cx, 5, command.as_bytes(), 1);
    graphics_layout_wait_image(&view, cx, 3001);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = images
            .iter()
            .find(|image| image.placement.image_id == 3001)
            .unwrap();
        let bytes = image.data.as_ref().unwrap().as_bytes(0).unwrap();
        assert_eq!(bytes.len(), 192 * 128 * 4);
        assert!(bytes.chunks_exact(4).all(|pixel| pixel == [0, 0, 255, 255]));
        assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
        assert!(
            !image.evicted,
            "an ACKed source after a large text snapshot must remain paintable"
        );
    });
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_scrolled_history_survives_new_output_and_hidden_viewport_withdraws(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    for placeholder in [false, true] {
        let daemon = GraphicsDaemonProcess::start();
        let pane = daemon.spawn();
        let (view, window) =
            graphics_rooted_consumer(cx, pane, daemon.stream_with_capacity(pane, 1024, 8));
        let first = if placeholder {
            "\x1b[2J\x1b[3J\x1b_Ga=T,i=7,p=1,U=1,c=2,r=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[6;5H\x1b[38;5;7;58;5;1m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m"
        } else {
            "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\"
        };
        graphics_send_fixture(&view, cx, 1, first.as_bytes());
        graphics_layout_wait_image(&view, cx, 7);
        graphics_send_fixture(&view, cx, 2, &b"\r\n".repeat(80));
        graphics_send_fixture(
            &view,
            cx,
            3,
            b"\x1b[6;5H\x1b_Ga=T,i=8,p=2,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\",
        );
        graphics_layout_wait_image(&view, cx, 8);
        graphics_send_fixture(&view, cx, 4, &b"\r\n".repeat(80));
        view.update(cx, |view, cx| {
            view.scroll(10_000, None, &Modifiers::default(), cx)
        });
        graphics_assert_production_paint(window, &view, cx);
        // New output advances history_origin before its next image is admitted.
        // The reported absolute top must continue to protect the old red pixels.
        let mut output = b"\r\n".repeat(80);
        output.extend_from_slice(
            b"\x1b[10;5H\x1b_Ga=T,i=9,p=3,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
        );
        graphics_send_fixture(&view, cx, 5, &output);
        graphics_layout_wait_image(&view, cx, 9);
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            let watched = images
                .iter()
                .find(|image| image.placement.image_id == 7)
                .unwrap();
            assert!(!watched.evicted);
            assert_eq!(
                watched.data.as_ref().unwrap().as_bytes(0).unwrap(),
                [0, 0, 255, 255]
            );
            let history = images
                .iter()
                .find(|image| image.placement.image_id == 8)
                .unwrap();
            assert!(
                history.evicted && history.data.is_none(),
                "only the non-visible history is reclaimed"
            );
        });
        graphics_assert_production_paint(window, &view, cx);
        cx.update(|cx| declare_displayed(cx, [(view.entity_id(), false)]));
        graphics_send_fixture(
            &view,
            cx,
            6,
            b"\x1b[12;5H\x1b_Ga=T,i=10,p=4,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/////w==\x1b\\",
        );
        graphics_layout_wait_image(&view, cx, 10);
        cx.update(|cx| declare_displayed(cx, [(view.entity_id(), true)]));
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            let old = images
                .iter()
                .find(|image| image.placement.image_id == 7)
                .unwrap();
            assert!(
                old.evicted && old.data.is_none(),
                "a hidden connection cannot pin its old viewport"
            );
            assert!(old.painted.load(std::sync::atomic::Ordering::Acquire));
        });
        daemon.panes().kill(pane).unwrap();
    }
}

#[gpui_kit::test]
fn graphics_consumer_hidden_live_pressure_refuses_until_last_live_placement_leaves(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    const GPU_PAGE: usize = 4 << 20;
    crate::terminal::images::with_test_cache_limits(GPU_PAGE + 1024, GPU_PAGE + 1024, || {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
        });
        for remove in [false, true] {
            let owner = GraphicsDaemonProcess::start();
            let requester = GraphicsDaemonProcess::start();
            let old_pane = owner.spawn();
            let new_pane = requester.spawn();
            let (old, old_window) = graphics_rooted_consumer(cx, old_pane, owner.stream(old_pane));
            let (new, new_window) =
                graphics_rooted_consumer(cx, new_pane, requester.stream(new_pane));
            graphics_send_fixture(&old, cx, 1, b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[24;5H\x1b_Ga=p,i=7,p=2,c=2,r=1,C=1,q=2\x1b\\");
            graphics_layout_wait_image(&old, cx, 7);
            graphics_assert_production_paint(old_window, &old, cx);
            cx.update(|cx| declare_displayed(cx, [(old.entity_id(), false)]));
            old_window
                .update(cx, |_, window, cx| {
                    window.replace_root(cx, |_, _| gpui_kit::EmptyView);
                })
                .unwrap();
            graphics_send_fixture(&old, cx, 2, b"\x1b[0m");
            let upload = b"\x1b[6;5H\x1b_Ga=T,i=8,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\";
            graphics_send_fixture(&new, cx, 3, upload);
            assert!(
                new.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
                "the daemon legitimately protects hidden content still in the live grid"
            );
            graphics_send_fixture(&old, cx, 4, &b"\r\n".repeat(10));
            old.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert!(images.iter().any(|image| image.placement.live));
                assert!(images.iter().any(|image| !image.placement.live));
                assert!(images.iter().all(|image| !image.evicted));
            });
            graphics_send_fixture(&new, cx, 5, upload);
            assert!(
                new.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
                "one historical placement cannot unprotect another live reference"
            );
            if remove {
                graphics_send_fixture(&old, cx, 6, b"\x1b_Ga=d,d=i,i=7\x1b\\");
            } else {
                graphics_send_fixture(&old, cx, 6, &b"\r\n".repeat(80));
            }
            old.read_with(cx, |view, _| {
                assert!(
                    graphics_visible_images(view)
                        .iter()
                        .all(|image| !image.placement.live)
                )
            });
            graphics_send_fixture(&new, cx, 7, upload);
            assert!(
                new.read_with(cx, |view, _| graphics_visible_images(view)
                    .iter()
                    .any(|image| image.placement.image_id == 8)),
                "a refused pressure request must become retryable after authoritative live protection ends"
            );
            graphics_layout_wait_image(&new, cx, 8);
            old.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert_eq!(images.len(), if remove { 0 } else { 2 });
                assert!(
                    images
                        .iter()
                        .all(|image| image.evicted && image.data.is_none()),
                    "authoritative eviction retains both historical notice positions"
                );
            });
            owner.panes().kill(old_pane).unwrap();
            requester.panes().kill(new_pane).unwrap();
            graphics_recovery_destroy(old, old_window, cx);
            graphics_recovery_destroy(new, new_window, cx);
        }
    });
}

#[gpui_kit::test]
fn graphics_consumer_preempted_scroll_viewport_cannot_pin_replayed_history(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (prior, prior_window) =
        graphics_rooted_consumer(cx, pane, daemon.stream_with_capacity(pane, 1024, 8));
    graphics_send_fixture(
        &prior,
        cx,
        1,
        b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
    );
    graphics_layout_wait_image(&prior, cx, 7);
    graphics_send_fixture(&prior, cx, 2, &b"\r\n".repeat(80));
    graphics_send_fixture(
        &prior,
        cx,
        3,
        b"\x1b[6;5H\x1b_Ga=T,i=8,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\",
    );
    graphics_layout_wait_image(&prior, cx, 8);
    graphics_send_fixture(&prior, cx, 4, &b"\r\n".repeat(80));
    prior.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_send_fixture(&prior, cx, 5, b"old_viewport_was_processed");
    // A new production attach preempts the old controller. Neither its old
    // scroll position nor an in-flight sender clone is a current window fact.
    let (view, window) =
        graphics_rooted_consumer(cx, pane, daemon.stream_with_capacity(pane, 1024, 8));
    graphics_send_fixture(
        &view,
        cx,
        6,
        b"\x1b[10;5H\x1b_Ga=T,i=9,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
    );
    graphics_layout_wait_image(&view, cx, 9);
    view.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let oldest = images
            .iter()
            .find(|image| image.placement.image_id == 7)
            .unwrap();
        assert!(oldest.evicted && oldest.data.is_none());
        let retained = images
            .iter()
            .find(|image| image.placement.image_id == 8)
            .unwrap();
        assert!(!retained.evicted);
        assert_eq!(
            retained.data.as_ref().unwrap().as_bytes(0).unwrap(),
            [255, 0, 0, 255]
        );
    });
    prior_window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_inactive_main_history_yields_to_alt_without_reclaiming_last_live_pixels(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    const GPU_PAGE: usize = 4 << 20;
    crate::terminal::images::with_test_cache_limits(
        2 * GPU_PAGE + 1024,
        2 * GPU_PAGE + 1024,
        || {
            for placeholder in [false, true] {
                let daemon = GraphicsDaemonProcess::start();
                let pane = daemon.spawn();
                let (view, window) =
                    graphics_rooted_consumer(cx, pane, daemon.stream_with_capacity(pane, 1024, 8));
                let first = if placeholder {
                    "\x1b[2J\x1b[3J\x1b_Ga=T,i=7,p=1,U=1,c=2,r=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[6;5H\x1b[38;5;7;58;5;1m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m"
                } else {
                    "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\"
                };
                graphics_send_fixture(&view, cx, 1, first.as_bytes());
                graphics_layout_wait_image(&view, cx, 7);
                graphics_send_fixture(&view, cx, 2, &b"\r\n".repeat(80));
                graphics_send_fixture(
                    &view,
                    cx,
                    3,
                    b"\x1b[6;5H\x1b_Ga=T,i=8,p=2,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\",
                );
                graphics_layout_wait_image(&view, cx, 8);
                graphics_send_fixture_with_replies(&view, cx, 4, b"\x1b[?1049h\x1b[6;5H\x1b_Ga=T,i=9,p=3,c=2,r=1,C=1,f=32,s=1,v=1;AP8A/w==\x1b\\", 1);
                graphics_layout_wait_image(&view, cx, 9);
                graphics_assert_production_paint(window, &view, cx);
                view.read_with(cx, |view, _| {
                    let images = graphics_visible_images(view);
                    assert_eq!(images.len(), 1);
                    assert_eq!(images[0].placement.image_id, 9);
                    assert_eq!(
                        images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                        [0, 255, 0, 255]
                    );
                });
                graphics_send_fixture(&view, cx, 5, b"\x1b[?1049l");
                graphics_layout_wait_image(&view, cx, 8);
                view.read_with(cx, |view, _| {
                    let images = graphics_visible_images(view);
                    let live = images
                        .iter()
                        .find(|image| image.placement.image_id == 8)
                        .unwrap();
                    assert!(!live.evicted);
                    assert_eq!(
                        live.data.as_ref().unwrap().as_bytes(0).unwrap(),
                        [255, 0, 0, 255]
                    );
                });
                view.update(cx, |view, cx| {
                    view.scroll(10_000, None, &Modifiers::default(), cx)
                });
                graphics_assert_production_paint(window, &view, cx);
                view.read_with(cx, |view, _| {
                    let images = graphics_visible_images(view);
                    let history = images
                        .iter()
                        .find(|image| image.placement.image_id == 7)
                        .unwrap();
                    assert!(history.evicted && history.data.is_none());
                    assert!(
                        history.painted.load(std::sync::atomic::Ordering::Acquire),
                        "the retained main history paints its notice, not resurrected pixels"
                    );
                    assert!(
                        !images.iter().any(|image| image.placement.image_id == 9),
                        "alternate resources never leak back into main"
                    );
                });
                graphics_recovery_destroy(view, window, cx);
                daemon.panes().kill(pane).unwrap();
            }
        },
    );
}

#[gpui_kit::test]
fn graphics_consumer_contended_viewport_retries_refused_history_without_new_output(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    const GPU_PAGE: usize = 4 << 20;
    crate::terminal::images::with_test_cache_limits(GPU_PAGE + 1024, GPU_PAGE + 1024, || {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
        });
        let owner = GraphicsDaemonProcess::start();
        let requester = GraphicsDaemonProcess::start();
        let old_pane = owner.spawn();
        let new_pane = requester.spawn();
        let (old, old_window) = graphics_rooted_consumer(cx, old_pane, owner.stream(old_pane));
        let (new, new_window) = graphics_rooted_consumer(cx, new_pane, requester.stream(new_pane));
        // Keep the production consumer alive without another paint/scrollbar
        // report concealing the one contended report below. No atlas lease is
        // acquired for the image, so retirement does not require a new frame.
        old_window
            .update(cx, |_, window, cx| {
                window.replace_root(cx, |_, _| gpui_kit::EmptyView);
            })
            .unwrap();
        graphics_send_fixture(
            &old,
            cx,
            1,
            b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
        );
        graphics_layout_wait_image(&old, cx, 7);
        graphics_send_fixture(&old, cx, 2, &b"\r\n".repeat(80));
        old.update(cx, |view, cx| {
            view.scroll(10_000, None, &Modifiers::default(), cx)
        });
        graphics_send_fixture(&old, cx, 3, b"viewport_barrier");
        let (term, reporter) = old.read_with(cx, |view, _| {
            (
                view.terminal.term.clone(),
                view.terminal.viewport_reporter(),
            )
        });
        // The scroll has changed the real grid but its report has not run yet.
        // Admission now sees historical pixels while the authority still sees
        // the old viewport, and must refuse without destroying protected red.
        term.lock().scroll_display(Scroll::Bottom);
        let upload = b"\x1b[6;5H\x1b_Ga=T,i=8,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\";
        graphics_send_fixture(&new, cx, 4, upload);
        assert!(new.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
        old.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            assert_eq!(images.len(), 1);
            assert!(!images[0].evicted);
            assert_eq!(
                images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                [0, 0, 255, 255]
            );
        });
        // Deterministically contend at the existing reporter/grid seam. There
        // is no later old-pane text, Place/Remove, repaint, or report to rescue
        // this update. It must converge after the held grid becomes available.
        let grid = term.lock();
        reporter.report();
        drop(grid);
        graphics_send_fixture(&new, cx, 5, upload);
        assert!(
            new.read_with(cx, |view, _| graphics_visible_images(view)
                .iter()
                .any(|image| image.placement.image_id == 8)),
            "a contended viewport and an earlier pressure refusal must not permanently pin history"
        );
        graphics_layout_wait_image(&new, cx, 8);
        graphics_assert_production_paint(new_window, &new, cx);
        old.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            assert_eq!(images.len(), 1);
            assert!(images[0].evicted && images[0].data.is_none());
        });
        owner.panes().kill(old_pane).unwrap();
        requester.panes().kill(new_pane).unwrap();
        drop((term, reporter));
        graphics_recovery_destroy(old, old_window, cx);
        graphics_recovery_destroy(new, new_window, cx);
    });
}

mod opencode_replay_consumer_tests {
    include!("opencode_replay_consumer_tests.rs");
}
include!("omp_replay_consumer_tests.rs");

// Observe the cells/caret consumed by the production Element, not its live VT
// grid or notification channel. A real stream pair keeps reader deadlines live.
fn sync_presented_row(view: &TerminalView, row: usize) -> String {
    let cols = view.cell_geom.expect("a production frame was painted").cols;
    view.grid_buf[row * cols..(row + 1) * cols]
        .iter()
        .filter(|cell| !cell.spacer)
        .flat_map(|cell| {
            std::iter::once(cell.c).chain(cell.marks.as_deref().unwrap_or(&[]).iter().copied())
        })
        // Packed untouched cells paint as blanks. Preserve their columns;
        // only the caller deciding a logical range may trim trailing padding.
        .map(|ch| if ch == '\0' { ' ' } else { ch })
        .collect()
}

#[gpui_kit::test]
fn synchronized_frame_holds_text_and_caret_across_unrelated_root_redraws(cx: &mut TestAppContext) {
    use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, WireFormat};
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, mut sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        171,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    DaemonMsg::Output("\x1b[2J\x1b[Hcommitted\x1b[2;1H宽e\u{301}\x1b[3;1H\x1b]8;;https://old.example/x\x1b\\visible-link\x1b]8;;\x1b\\\x1b[3;5H".as_bytes().to_vec())
        .encode(&mut sender).unwrap();
    let images = view.read_with(cx, |view, _| view.terminal.images());
    send_admitted_graphics(
        &images,
        &mut sender,
        GraphicsUpdate::Resource {
            key: 171,
            image: std::sync::Arc::new(Image {
                width: 1,
                height: 1,
                format: WireFormat::Rgba,
                compressed: false,
                data: vec![255, 0, 0, 255],
            }),
        },
    );
    let mut placement = graphics_placement(172, 171, 4, 3);
    placement.x = 0;
    send_admitted_graphics(&images, &mut sender, GraphicsUpdate::Place(placement));
    send_graphics(
        &mut sender,
        GraphicsUpdate::Screen {
            screen: 0,
            history_origin: 0,
            epoch: 0,
        },
    );
    graphics_recovery_wait(cx, "the initial committed frame was not painted", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap
                .as_ref()
                .is_some_and(|snap| snap.images.len() == 1)
                && sync_presented_row(view, 0).starts_with("committed")
        })
    });
    let painted = view.read_with(cx, |view, _| {
        view.grid_snap.as_ref().unwrap().images[0].painted.clone()
    });
    // A protocol query after the replacement proves bytes are parsed inside
    // the hold; it also distinguishes the removed parser-buffering behavior.
    sender
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    DaemonMsg::Output(b"\x1b[?2026h".to_vec())
        .encode(&mut sender)
        .unwrap();
    // A non-output frame flushes BSU before the next parser advance.
    DaemonMsg::Cwd(std::path::PathBuf::from("/sync-fixture"))
        .encode(&mut sender)
        .unwrap();
    DaemonMsg::Output(b"\x1b[Hpartial\x1b[K\x1b[2;1Hhidden\x1b[K\x1b[3;1H\x1b]8;;https://new.example/x\x1b\\visible-link\x1b]8;;\x1b\\\x1b[7;9H".to_vec()).encode(&mut sender).unwrap();
    send_graphics(
        &mut sender,
        GraphicsUpdate::Remove {
            placements: vec![172],
            resources: vec![171],
        },
    );
    DaemonMsg::Output(b"\x1b[6n".to_vec())
        .encode(&mut sender)
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let parsed = view.read_with(cx, |view, _| {
            let mut term = view.terminal.term.lock();
            if term.grid[Line(0)][Column(0)].c() != 'p' {
                return false;
            }
            assert!(
                term.synchronized_output(),
                "the pre-deadline observation must still be held"
            );
            // Control only this atomicity test's clock origin. The separate
            // quiet-stream regression uses the real 150 ms reader deadline.
            term.sync_update_started =
                Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(5));
            true
        });
        if parsed {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live parsing stalled behind BSU"
        );
        std::thread::yield_now();
    }
    let mut replies = sender.try_clone().unwrap();
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        loop {
            if let ClientMsg::Input(bytes) = ClientMsg::read(&mut replies).unwrap() {
                reply_tx.send(bytes).unwrap();
                break;
            }
        }
    });
    let mut reply = None;
    graphics_recovery_wait(cx, "DSR did not advance inside a presentation hold", |_| {
        reply = reply_rx.try_recv().ok();
        reply.is_some()
    });
    reader.join().unwrap();
    assert_eq!(reply.unwrap(), b"\x1b[7;9R");
    painted.store(false, std::sync::atomic::Ordering::Release);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.focus_handle.focus(window, cx);
                cx.notify();
            });
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    view.read_with(cx, |view, _| {
        assert!(sync_presented_row(view, 0).starts_with("committed"));
        assert_eq!(view.grid_snap.as_ref().unwrap().painted_cursor().map(|(r,c,_)| (r,c)), Some((2,4)));
        assert!(view.scrollback_text().contains("partial"), "live parsing must advance ahead of the held picture");
        let image = &view.grid_snap.as_ref().unwrap().images[0];
        assert_eq!((image.anchor_row, image.anchor_col), (4, 3));
        assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), &[0, 0, 255, 255]);
        assert!(painted.load(std::sync::atomic::Ordering::Acquire), "the unrelated redraw must really paint the old image after its live resource was removed");
        assert!(view.terminal.images().snapshot().is_empty(), "daemon-owned live removal must already be applied");
    });
    view.update(cx, |view, cx| {
        view.on_select_start(0, 0, true, 1, false, cx);
        view.on_select_update(8, 0, false, cx);
        view.on_select_end(cx);
        view.copy_selection(cx);
        assert_eq!(view.selection_text().as_deref(), Some("committed"));
    });
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("committed")
    );
    view.update(cx, |view, cx| {
        view.on_select_start(1, 1, true, 1, false, cx);
        view.on_select_update(2, 1, false, cx);
        view.on_select_end(cx);
        view.copy_selection(cx);
    });
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("宽e\u{301}")
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| assert!(view.open_link_at(2, 2, window, cx)));
        })
        .unwrap();
    assert_eq!(cx.opened_url().as_deref(), Some("https://old.example/x"));
    DaemonMsg::Output(b"\x1b[?2026l".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "ESU did not publish the coherent frame", |cx| {
        view.read_with(cx, |view, _| {
            sync_presented_row(view, 0).starts_with("partial")
                && view.grid_snap.as_ref().unwrap().images.is_empty()
                && view
                    .grid_snap
                    .as_ref()
                    .unwrap()
                    .painted_cursor()
                    .map(|(r, c, _)| (r, c))
                    == Some((6, 8))
        })
    });
}

#[gpui_kit::test]
fn quiet_unfinished_synchronized_frame_paints_without_more_output(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, mut sender) = test_stream_pair();
    let (view, _window) = graphics_build_rooted_consumer_at(
        cx,
        172,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    DaemonMsg::Output(b"\x1b[?2026h\x1b[Hquiet-first-frame".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(
        cx,
        "a quiet first synchronized frame remained suspended",
        |cx| {
            view.read_with(cx, |view, _| {
                view.grid_snap.is_some()
                    && sync_presented_row(view, 0).starts_with("quiet-first-frame")
            })
        },
    );
    view.read_with(cx, |view, _| {
        assert!(!view.terminal.term.lock().synchronized_output())
    });
    DaemonMsg::Output(b"\r\nafter-timeout".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "output after timeout was lost", |cx| {
        view.read_with(cx, |view, _| {
            sync_presented_row(view, 1).starts_with("after-timeout")
        })
    });
}

#[gpui_kit::test]
fn presentation_boundaries_publish_received_text_without_a_later_pty_frame(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::WinSize;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let cases = [
        ("replay", DaemonMsg::Snapshot(Vec::new()), "received"),
        (
            "resize",
            DaemonMsg::Size(WinSize {
                cols: 79,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            }),
            "received",
        ),
        ("reset", DaemonMsg::Output(b"\x1bcreset".to_vec()), "reset"),
        (
            "command",
            DaemonMsg::Output(b"\x1b]133;D;0\x07".to_vec()),
            "received",
        ),
        (
            "prompt",
            DaemonMsg::Prompt {
                active: true,
                at_prompt: true,
                last_exit: Some(0),
            },
            "received",
        ),
        ("exit", DaemonMsg::Exited { code: Some(0) }, "received"),
    ];
    for (label, boundary, expected) in cases {
        let (client, mut sender) = test_stream_pair();
        let (view, window) = graphics_build_rooted_consumer_at(
            cx,
            173,
            client,
            Vec::new(),
            TermSize::new(80, 24),
            PtySource::Raw,
        );
        DaemonMsg::Output(b"\x1b[?2026h\x1b[Hreceived".to_vec())
            .encode(&mut sender)
            .unwrap();
        boundary.encode(&mut sender).unwrap();
        graphics_recovery_wait(cx, label, |cx| {
            view.read_with(cx, |view, _| {
                view.grid_snap.is_some()
                    && sync_presented_row(view, 0).starts_with(expected)
                    && !view.terminal.term.lock().synchronized_output()
            })
        });
        graphics_recovery_destroy(view, window, cx);
    }
}

#[gpui_kit::test]
fn reconnect_replay_replaces_an_open_picture_without_inheriting_its_hold(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, mut old_sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        174,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    DaemonMsg::Output(b"\x1b[Hbefore-reconnect".to_vec())
        .encode(&mut old_sender)
        .unwrap();
    graphics_recovery_wait(cx, "baseline", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some() && sync_presented_row(view, 0).starts_with("before-reconnect")
        })
    });
    DaemonMsg::Output(b"\x1b[?2026h\x1b[Hunfinished-old-link".to_vec())
        .encode(&mut old_sender)
        .unwrap();
    let (client, mut sender) = test_stream_pair();
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
    DaemonMsg::Snapshot(b"\x1b[?2026h\x1b[Hreplayed-new-link\x1b[6n".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "fresh replay kept an old presentation hold", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some()
                && sync_presented_row(view, 0).starts_with("replayed-new-link")
                && !view.terminal.term.lock().synchronized_output()
        })
    });
    DaemonMsg::Output(b"\r\nnew-live-output".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "live output after replay", |cx| {
        view.read_with(cx, |view, _| {
            sync_presented_row(view, 1).starts_with("new-live-output")
        })
    });
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn gui_and_cli_shared_synchronized_ranges_converge_at_the_same_geometry(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let source = if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    };
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        pane,
        daemon.stream(pane),
        Vec::new(),
        TermSize::new(40, 24),
        source,
    );
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    graphics_send_fixture(
        &view,
        cx,
        171,
        &sync_capture_fixture::synchronized_capture_frame(),
    );
    let visible_range = |view: &TerminalView, first: usize| {
        format!(
            "{}{}",
            sync_presented_row(view, first).trim_end(),
            sync_presented_row(view, first + 1).trim_end()
        )
    };
    graphics_recovery_wait_with(
        cx,
        std::time::Duration::from_secs(30),
        |cx| {
            view.read_with(cx, |view, _| {
                view.grid_snap.is_some()
                    && visible_range(view, 3) == sync_capture_fixture::SYNC_CAPTURE_TEXT
            })
        },
        |cx| {
            view.read_with(cx, |view, _| {
            let term = view.terminal.term.lock();
            format!("quiet GUI did not converge to the CLI's shared Unicode/wrap range; expected={:?}; painted_rows={:?}; painted_geometry={:?}; live_geometry={}x{}; live_wrap={}; synchronized={}",
                sync_capture_fixture::SYNC_CAPTURE_TEXT,
                view.grid_snap.as_ref().map(|_| (sync_presented_row(view, 3), sync_presented_row(view, 4))),
                view.cell_geom.map(|geometry| (geometry.cols, geometry.rows)),
                term.columns(), term.screen_lines(), term.mode().contains(rio_vt::crosswords::Mode::LINE_WRAP), term.synchronized_output())
        })
        },
    );
    graphics_send_fixture(&view, cx, 172, b"\x1b[?2026l");
    graphics_recovery_wait(
        cx,
        "settled GUI frame diverged from the shared capture range",
        |cx| {
            view.read_with(cx, |view, _| {
                visible_range(view, 3) == sync_capture_fixture::SYNC_CAPTURE_TEXT
                    && view
                        .grid_snap
                        .as_ref()
                        .unwrap()
                        .painted_cursor()
                        .map(|(r, c, _)| (r, c))
                        == Some((4, 22))
            })
        },
    );
    graphics_send_fixture(
        &view,
        cx,
        173,
        format!(
            "\x1b[?2026h\x1b[8;1H{}\x1b[?2026l",
            sync_capture_fixture::SYNC_NEXT_TEXT
        )
        .as_bytes(),
    );
    graphics_recovery_wait(
        cx,
        "successive GUI update lost the shared capture range",
        |cx| {
            view.read_with(cx, |view, _| {
                visible_range(view, 7) == sync_capture_fixture::SYNC_NEXT_TEXT
                    && view
                        .grid_snap
                        .as_ref()
                        .unwrap()
                        .painted_cursor()
                        .map(|(r, c, _)| (r, c))
                        == Some((8, 6))
            })
        },
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

fn synchronized_resize_consumer(cx: &mut TestAppContext, finish_before_size: Option<bool>) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, _initial_sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        175,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    let (client, mut sender) = test_stream_pair();
    view.update(cx, |view, cx| {
        view.adopt_relink(
            client,
            Vec::new(),
            &crate::terminal::PaneRoute::Remote {
                header: Box::new(crate::daemon::router::RouteHeader::wsl("resize-fixture")),
                resize_echo: true,
            },
            TermSize::new(80, 24),
            10,
            20,
            cx,
        )
        .unwrap();
    });
    DaemonMsg::Output(b"\x1b[2J\x1b[Hbefore\x1b[2;1Hrow-two\x1b[3;5H".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "committed rows were not painted", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some() && sync_presented_row(view, 1).starts_with("row-two")
        })
    });
    DaemonMsg::Output(b"\x1b[?2026h".to_vec())
        .encode(&mut sender)
        .unwrap();
    DaemonMsg::Cwd(std::path::PathBuf::from("/resize-hold"))
        .encode(&mut sender)
        .unwrap();
    DaemonMsg::Output(b"\x1b[Hpartial".to_vec())
        .encode(&mut sender)
        .unwrap();
    // The reader remains real; move only this test's deadline so its held
    // picture can be observed independently of the 150 ms quiet-stream case.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if view.read_with(cx, |view, _| {
            let mut term = view.terminal.term.lock();
            if term.grid[Line(0)][Column(0)].c() != 'p' {
                return false;
            }
            assert!(term.synchronized_output());
            term.sync_update_started =
                Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(30));
            true
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "hold was not parsed");
        std::thread::yield_now();
    }
    let target = graphics_window_geometry(window, &view, cx, TermSize::new(96, 20)).0;
    cx.simulate_window_resize(window, target);
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    view.read_with(cx, |view, _| {
        assert_eq!(
            sync_presented_row(view, 1).trim_end(),
            "row-two",
            "equal area resize must not reinterpret the committed second row"
        );
        let geom = view.cell_geom.unwrap();
        assert_eq!((geom.cols, geom.rows), (80, 24));
        assert_eq!(
            view.grid_snap
                .as_ref()
                .unwrap()
                .painted_cursor()
                .map(|(r, c, _)| (r, c)),
            Some((2, 4))
        );
    });
    view.update(cx, |view, cx| {
        view.on_select_start(0, 1, true, 1, false, cx);
        view.on_select_update(6, 1, false, cx);
        view.on_select_end(cx);
        view.copy_selection(cx);
    });
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("row-two")
    );
    if let Some(timeout) = finish_before_size {
        if timeout {
            view.read_with(cx, |view, _| {
                view.terminal.term.lock().sync_update_started =
                    Some(rio_vt::time::Instant::now() - std::time::Duration::from_secs(1));
            });
        } else {
            DaemonMsg::Output(b"\x1b[?2026l".to_vec())
                .encode(&mut sender)
                .unwrap();
        }
        graphics_recovery_wait(cx, "hold did not end before the ordered Size", |cx| {
            view.read_with(cx, |view, _| {
                !view.terminal.term.lock().synchronized_output()
            })
        });
        window
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        view.read_with(cx, |view, _| {
            assert_eq!(
                sync_presented_row(view, 1).trim_end(),
                "row-two",
                "ending the hold must not readdress old-width VT cells"
            );
            let geom = view.cell_geom.unwrap();
            assert_eq!((geom.cols, geom.rows), (80, 24));
            assert!(
                sync_presented_row(view, 0).starts_with("before"),
                "pending resize must retain one coherent committed picture"
            );
        });
    }
    DaemonMsg::Size(crate::daemon::protocol::WinSize {
        cols: 96,
        rows: 20,
        cell_w: 10,
        cell_h: 20,
    })
    .encode(&mut sender)
    .unwrap();
    graphics_recovery_wait(
        cx,
        "ordered resize did not release the old addressing",
        |cx| {
            view.read_with(cx, |view, _| {
                view.cell_geom
                    .is_some_and(|geom| (geom.cols, geom.rows) == (96, 20))
                    && !view.grid_snap.as_ref().unwrap().held
            })
        },
    );
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn synchronized_equal_area_resize_keeps_committed_cell_addressing(cx: &mut TestAppContext) {
    synchronized_resize_consumer(cx, None);
}

#[gpui_kit::test]
fn synchronized_esu_before_size_keeps_committed_cell_addressing(cx: &mut TestAppContext) {
    synchronized_resize_consumer(cx, Some(false));
}

#[gpui_kit::test]
fn synchronized_timeout_before_size_keeps_committed_cell_addressing(cx: &mut TestAppContext) {
    synchronized_resize_consumer(cx, Some(true));
}

#[gpui_kit::test]
fn synchronized_copy_preserves_live_tab_line_wrap_and_unicode_semantics(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, mut sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        176,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    DaemonMsg::Output("\x1b[?7h\x1b[2J\x1b[Ha\tb\x1b[2;1Hplain  text\x1b[3;79H界e\u{301}Z\x1b[5;1HAe\u{301}中B\x1b[7;1Ha\x1b[10;1Hb\x1b[11;1Htail   suffix\x1b[12;1Htrail   \x1b[13;80H中e\u{301}B\x1b[15;1H   indented\x1b[16;1Hhard-a\x1b[17;1Hhard-b\x1b[21;78Ha   b".as_bytes().to_vec())
        .encode(&mut sender).unwrap();
    graphics_recovery_wait(cx, "copy fixture did not paint", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some() && sync_presented_row(view, 4).starts_with("Ae\u{301}中B")
        })
    });
    let wrapped_line = format!("{}界e\u{301}Z\n", " ".repeat(78));
    let leading_wrapped_line = format!("\n{}中e\u{301}B\n", " ".repeat(79));
    let wrapped_space_line = format!("{}a   b\n", " ".repeat(77));
    let cases = [
        ("tab", (0, 0), (8, 0), 1, "a\tb"),
        ("line", (0, 0), (8, 0), 3, "a\tb\n"),
        (
            "tab filler selection starts after Tab",
            (2, 0),
            (8, 0),
            1,
            "      b",
        ),
        ("selection ends within tab run", (0, 0), (3, 0), 1, "a\t"),
        ("plain", (0, 1), (10, 1), 1, "plain  text"),
        ("wrap", (78, 2), (1, 3), 1, "界e\u{301}Z"),
        ("wrapped Lines", (78, 2), (1, 3), 3, wrapped_line.as_str()),
        (
            "soft wrap preserves cross-row spaces",
            (77, 20),
            (1, 21),
            1,
            "a   b",
        ),
        (
            "Lines soft wrap preserves cross-row spaces",
            (77, 20),
            (1, 21),
            3,
            wrapped_space_line.as_str(),
        ),
        (
            "wide and combining boundaries",
            (1, 4),
            (3, 4),
            1,
            "e\u{301}中",
        ),
        ("starts at wide trailing spacer", (3, 4), (4, 4), 1, "中B"),
        (
            "ends at wide trailing spacer",
            (0, 4),
            (3, 4),
            1,
            "Ae\u{301}中",
        ),
        ("combining cell only", (1, 4), (1, 4), 1, "e\u{301}"),
        ("Simple trailing blank rows", (0, 6), (79, 8), 1, "a"),
        ("Lines trailing blank rows", (0, 6), (79, 8), 3, "a\n"),
        ("embedded blank rows stay", (0, 6), (0, 9), 1, "a\n\n\nb"),
        (
            "Rio trims selected trailing spaces",
            (0, 10),
            (6, 10),
            1,
            "tail",
        ),
        (
            "right padding is not text",
            (0, 10),
            (79, 10),
            1,
            "tail   suffix",
        ),
        ("terminal trailing spaces", (0, 11), (79, 11), 1, "trail"),
        (
            "starts at wide leading spacer",
            (79, 12),
            (3, 13),
            1,
            "\n中e\u{301}B",
        ),
        (
            "Lines starts at wide leading spacer",
            (79, 12),
            (3, 13),
            3,
            leading_wrapped_line.as_str(),
        ),
        (
            "Lines ends at wide leading spacer",
            (78, 12),
            (79, 12),
            3,
            leading_wrapped_line.as_str(),
        ),
        (
            "leading whitespace stays",
            (0, 14),
            (10, 14),
            1,
            "   indented",
        ),
        ("hard line break", (0, 15), (5, 16), 1, "hard-a\nhard-b"),
        ("Simple all blank rows", (0, 17), (79, 18), 1, ""),
        ("Lines all blank rows", (0, 17), (79, 18), 3, "\n"),
    ];
    // Establish Rio's ordinary consumer result against independent literals
    // before any hidden state exists, not by copying the live partial grid.
    let mut live_copies = Vec::new();
    let mut failures = Vec::new();
    for &(label, (start_col, start_row), (end_col, end_row), clicks, expected) in &cases {
        view.update(cx, |view, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                "copy-sentinel".to_owned(),
            ));
            view.on_select_start(start_col, start_row, true, clicks, false, cx);
            view.on_select_update(end_col, end_row, false, cx);
            view.on_select_end(cx);
            view.copy_selection(cx);
        });
        let clipboard_expected = if expected.is_empty() {
            "copy-sentinel"
        } else {
            expected
        };
        let live = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        if live.as_deref() != Some(clipboard_expected) {
            failures.push(format!(
                "live {label}: literal={expected:?}, clipboard={live:?}"
            ));
        }
        live_copies.push(live);
    }
    // Preserve a live Lines selection in the committed picture as well as
    // exercising Lines gestures started after the hold below.
    view.update(cx, |view, cx| {
        view.on_select_start(0, 0, true, 3, false, cx);
        view.on_select_end(cx);
    });
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    DaemonMsg::Output(b"\x1b[?2026h".to_vec())
        .encode(&mut sender)
        .unwrap();
    DaemonMsg::Cwd(std::path::PathBuf::from("/copy-hold"))
        .encode(&mut sender)
        .unwrap();
    DaemonMsg::Output(b"\x1b[2J\x1b[Hhidden-partial".to_vec())
        .encode(&mut sender)
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if view.read_with(cx, |view, _| {
            let mut term = view.terminal.term.lock();
            if term.grid[Line(0)][Column(0)].c() != 'h' {
                return false;
            }
            assert!(term.synchronized_output());
            term.sync_update_started =
                Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(30));
            true
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "hidden copy fixture was not parsed"
        );
        std::thread::yield_now();
    }
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    view.update(cx, |view, cx| view.copy_selection(cx));
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    if copied.as_deref() != Some("a\tb\n") {
        failures.push(format!("committed live Lines: {copied:?}"));
    }
    for (case_index, &(label, (start_col, start_row), (end_col, end_row), clicks, expected)) in
        cases.iter().enumerate()
    {
        view.update(cx, |view, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                "copy-sentinel".to_owned(),
            ));
            view.on_select_start(start_col, start_row, true, clicks, false, cx);
            view.on_select_update(end_col, end_row, false, cx);
            view.on_select_end(cx);
            view.copy_selection(cx);
        });
        let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        if copied != live_copies[case_index] {
            failures.push(format!(
                "held {label}: literal={expected:?}, live={:?}, held={copied:?}",
                live_copies[case_index]
            ));
        }
    }
    // Rio's pre-existing end-LeadingSpacer copy quirk reads the preceding
    // row. The held contract copies only the visible selected placeholders,
    // not that unrelated character or the unselected next-row wide glyph.
    view.update(cx, |view, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "copy-sentinel".to_owned(),
        ));
        view.on_select_start(78, 12, true, 1, false, cx);
        view.on_select_update(79, 12, false, cx);
        view.on_select_end(cx);
        view.copy_selection(cx);
    });
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("copy-sentinel")
    );
    graphics_recovery_destroy(view, window, cx);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[gpui_kit::test]
fn synchronized_relink_first_hold_uses_new_empty_surface_geometry(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, _initial_sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        177,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    let route = crate::terminal::PaneRoute::Remote {
        header: Box::new(crate::daemon::router::RouteHeader::wsl(
            "first-hold-fixture",
        )),
        resize_echo: true,
    };
    let (client, mut old_sender) = test_stream_pair();
    view.update(cx, |view, cx| {
        view.adopt_relink(
            client,
            Vec::new(),
            &route,
            TermSize::new(80, 24),
            10,
            20,
            cx,
        )
        .unwrap()
    });
    DaemonMsg::Output(b"\x1b[Hprevious-link".to_vec())
        .encode(&mut old_sender)
        .unwrap();
    graphics_recovery_wait(cx, "old link did not paint", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some() && sync_presented_row(view, 0).starts_with("previous-link")
        })
    });
    let target = graphics_window_geometry(window, &view, cx, TermSize::new(40, 12)).0;
    cx.simulate_window_resize(window, target);
    let (client, mut sender) = test_stream_pair();
    view.update(cx, |view, cx| {
        assert_eq!(
            view.cell_geom.map(|geom| (geom.cols, geom.rows)),
            Some((80, 24))
        );
        view.adopt_relink(
            client,
            Vec::new(),
            &route,
            TermSize::new(40, 12),
            10,
            20,
            cx,
        )
        .unwrap();
        DaemonMsg::Output(b"\x1b[?2026h".to_vec())
            .encode(&mut sender)
            .unwrap();
        DaemonMsg::Cwd(std::path::PathBuf::from("/first-hold"))
            .encode(&mut sender)
            .unwrap();
        DaemonMsg::Output(b"\x1b[Hhidden-new-link".to_vec())
            .encode(&mut sender)
            .unwrap();
        // Stay in this one real consumer update until its production reader
        // parses the first BSU. No empty old-bounds frame may precede it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let mut term = view.terminal.term.lock();
            if term.grid[Line(0)][Column(0)].c() == 'h' {
                assert!(term.synchronized_output());
                term.sync_update_started =
                    Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(30));
                break;
            }
            drop(term);
            assert!(
                std::time::Instant::now() < deadline,
                "first held replay was not parsed"
            );
            std::thread::yield_now();
        }
        assert!(view.grid_snap.is_none());
        assert!(view.grid_buf.is_empty());
    });
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    view.read_with(cx, |view, _| {
        let geom = view.cell_geom.unwrap();
        assert_eq!((geom.cols, geom.rows), (40, 12));
        assert_eq!(view.grid_buf.len(), 40 * 12);
        assert!(
            sync_presented_row(view, 0).trim().is_empty(),
            "no previous picture exists on the new link"
        );
    });
    DaemonMsg::Size(crate::daemon::protocol::WinSize {
        cols: 40,
        rows: 12,
        cell_w: 10,
        cell_h: 20,
    })
    .encode(&mut sender)
    .unwrap();
    graphics_recovery_wait(cx, "new link did not settle", |cx| {
        view.read_with(cx, |view, _| {
            sync_presented_row(view, 0).starts_with("hidden-new-link")
        })
    });
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn synchronized_copy_preserves_tabstop_gaps_custom_clear_reset_and_growth(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let cases = [
        (
            "default stop gap",
            "",
            "a\t\x1b[1;12Hb",
            0,
            11,
            "a\t   b",
            false,
        ),
        (
            "custom HTS",
            "\x1b[1;6H\x1bH\x1b[H",
            "a\t\x1b[1;10Hb",
            0,
            9,
            "a\t    b",
            false,
        ),
        (
            "Rio no-op set_tabs preserves HTS",
            "\x1b[1;6H\x1bH\x1b[?5W\x1b[H",
            "a\t\x1b[1;10Hb",
            0,
            9,
            "a\t    b",
            false,
        ),
        (
            "clear current TBC",
            "\x1b[1;6H\x1bH\x1b[g\x1b[H",
            "a\t\x1b[1;12Hb",
            0,
            11,
            "a\t   b",
            false,
        ),
        (
            "clear all TBC",
            "\x1b[3g",
            "a\t\x1b[1;12Hb",
            0,
            11,
            "a\tb",
            false,
        ),
        (
            "RIS restores defaults",
            "\x1b[3g\x1bc",
            "a\t\x1b[1;12Hb",
            0,
            11,
            "a\t   b",
            false,
        ),
        (
            "resize adds default stops",
            "\x1b[3g\x1b[1;6H\x1bH",
            "\x1b[1;82Ha\t\x1b[1;92Hb",
            81,
            91,
            "a\t   b",
            true,
        ),
    ];
    let mut failures = Vec::new();
    for (label, setup, text, start, end, expected, grow) in cases {
        let (client, mut sender) = test_stream_pair();
        let size = TermSize::new(if grow { 96 } else { 80 }, 24);
        let (view, window) =
            graphics_build_rooted_consumer_at(cx, 178, client, Vec::new(), size, PtySource::Raw);
        if grow {
            DaemonMsg::Size(crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            })
            .encode(&mut sender)
            .unwrap();
        }
        DaemonMsg::Output(setup.as_bytes().to_vec())
            .encode(&mut sender)
            .unwrap();
        if grow {
            DaemonMsg::Size(crate::daemon::protocol::WinSize {
                cols: 96,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            })
            .encode(&mut sender)
            .unwrap();
        }
        DaemonMsg::Output(text.as_bytes().to_vec())
            .encode(&mut sender)
            .unwrap();
        graphics_recovery_wait(cx, "tab fixture did not paint", |cx| {
            view.read_with(cx, |view, _| {
                view.grid_snap.is_some() && view.grid_buf.get(end).is_some_and(|cell| cell.c == 'b')
            })
        });
        view.update(cx, |view, cx| {
            view.on_select_start(start, 0, true, 1, false, cx);
            view.on_select_update(end, 0, false, cx);
            view.on_select_end(cx);
            view.copy_selection(cx);
        });
        let live = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(live.as_deref(), Some(expected), "live Rio oracle: {label}");
        window
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        DaemonMsg::Output(b"\x1b[?2026h".to_vec())
            .encode(&mut sender)
            .unwrap();
        DaemonMsg::Cwd(std::path::PathBuf::from("/tab-hold"))
            .encode(&mut sender)
            .unwrap();
        // Hidden tab configuration must not replace the committed stop map.
        DaemonMsg::Output(b"\x1b[3g\x1b[1;4H\x1bH\x1b[Hhidden-partial".to_vec())
            .encode(&mut sender)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if view.read_with(cx, |view, _| {
                let mut term = view.terminal.term.lock();
                if term.grid[Line(0)][Column(0)].c() != 'h' {
                    return false;
                }
                assert!(term.synchronized_output());
                term.sync_update_started =
                    Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(30));
                true
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "hidden stop change was not parsed"
            );
            std::thread::yield_now();
        }
        window
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        view.update(cx, |view, cx| {
            view.on_select_start(start, 0, true, 1, false, cx);
            view.on_select_update(end, 0, false, cx);
            view.on_select_end(cx);
            view.copy_selection(cx);
        });
        let held = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        if held != live {
            failures.push(format!("{label}: live={live:?}, held={held:?}"));
        }
        graphics_recovery_destroy(view, window, cx);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
