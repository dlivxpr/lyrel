mod graphics_pty_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/kitty_pty.rs"
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
            if let DaemonMsg::Output(bytes) = session.recv().unwrap() {
                text.extend(bytes);
            }
        }
        let id = session.pane_id();
        session.detach().unwrap();
        id
    }

    fn stream(&self, pane: u64) -> Stream {
        let mut stream =
            crate::daemon::transport::connect_endpoint_at(&self.endpoint("daemon")).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
        stream.set_read_timeout(None).unwrap();
        ClientMsg::Attach {
            pane_id: pane,
            size: crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
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
    let request = format!("SEND {serial} 0 {} 0\n", graphics_pty_fixture::hex(bytes));
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let marker = format!("KITTY_DONE_{serial}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| view.scrollback_text().contains(&marker)) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "PTY round trip did not finish: {serial}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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
    let built = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    let window = cx.add_window(move |window, cx| {
        let terminal = RemoteTerminal::from_stream_with(
            stream,
            TermSize::new(80, 24),
            Vec::new(),
            if cfg!(windows) {
                PtySource::LocalConpty
            } else {
                PtySource::Raw
            },
        )
        .unwrap();
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = built.borrow_mut().take().unwrap();
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
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
        assert_eq!(images[0].data.as_bytes(0).unwrap(), &[0,0,255,255,0,255,0,255,255,0,0,255,255,255,255,255]);
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
        assert!(std::sync::Arc::ptr_eq(&images[0].data, &images[1].data));
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
                .any(|image| image.data.as_bytes(0).unwrap() == [255, 0, 0, 255])
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
    view.update(cx, |view, _| {
        view.terminal.resize(TermSize::new(40, 24), 15, 30)
    });
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
            std::sync::Arc::ptr_eq(&images[0].data, &prior.data),
            "multiple prototypes do not decode or copy their shared resource again"
        );
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    daemon.panes().kill(pane).unwrap();
}

fn graphics_retained_pane_clear(cx: &mut TestAppContext, child_exit: bool, history: bool) {
    use std::sync::{Arc, atomic::Ordering};

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
    let built = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    let window = cx.add_window(move |window, cx| {
        let terminal = RemoteTerminal::from_stream_with(
            stream,
            TermSize::new(80, 24),
            Vec::new(),
            if cfg!(windows) {
                PtySource::LocalConpty
            } else {
                PtySource::Raw
            },
        )
        .unwrap();
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = built.borrow_mut().take().unwrap();
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[3J\x1b[6;1HHISTORY-TEXT\x1b_Ga=T,i=101,f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| view.terminal.images().snapshot().len() == 1) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "history image was not decoded"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    graphics_assert_production_paint(window.into(), &view, cx);
    graphics_send_fixture(&view, cx, 2, &b"\r\n".repeat(60));
    graphics_send_fixture(
        &view,
        cx,
        3,
        b"\x1b[6;1HSCREEN-TEXT\x1b_Ga=T,i=102,f=32,s=1,v=1,c=1,r=1,C=1,q=2;AAD//w==\x1b\\",
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| view.terminal.images().snapshot().len() == 2) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "screen image was not decoded"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    graphics_assert_production_paint(window.into(), &view, cx);
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
        // Break the actual attached transport while the daemon and child live.
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
        let before: Vec<_> = images
            .iter()
            .map(|image| Arc::strong_count(&image.data))
            .collect();
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
        let retained = store.snapshot();
        assert_eq!(
            retained
                .iter()
                .map(|image| image.placement.image_id)
                .collect::<Vec<_>>(),
            if history { vec![] } else { vec![101] }
        );
        let retired = store.take_retired();
        for (image, count) in images.iter().zip(before) {
            if history || image.placement.image_id == 102 {
                assert!(
                    !store.claim_for_paint(image),
                    "cleared placements cannot claim cached paint snapshots"
                );
                assert!(
                    retired.iter().any(|data| Arc::ptr_eq(data, &image.data)),
                    "cleared painted resources must reach the atlas release queue"
                );
                assert_eq!(
                    Arc::strong_count(&image.data),
                    count,
                    "resource ownership moved to retirement instead of remaining in the cache"
                );
            }
        }
        drop(retired);
    });
    graphics_assert_production_paint(window.into(), &view, cx);
    // A disconnected pane is still a live daemon session, not a fabricated
    // exited flag. Its authoritative buffer was not cleared by the local action.
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
