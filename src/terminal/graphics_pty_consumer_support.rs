// Shared real PTY/daemon and rooted-window support for headless and native-font consumers.
struct GraphicsDaemonProcess {
    child: std::process::Child,
    dir: tempfile::TempDir,
}

impl GraphicsDaemonProcess {
    fn start() -> Self {
        Self::start_with(|_, _| {})
    }

    fn start_with(configure: impl FnOnce(&mut std::process::Command, &std::path::Path)) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
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
            .stderr(std::process::Stdio::null());
        configure(&mut command, dir.path());
        let child = command.spawn().unwrap();
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
        self.spawn_with_cwd(None)
    }

    fn spawn_with_cwd(&self, cwd: Option<std::path::PathBuf>) -> u64 {
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
                cwd,
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
        // Every consumer attached through this helper is a real production
        // TerminalView: it keeps the monochrome contract, so it declares the
        // render opt-in — the credential `q` routing and `s` answers bind to.
        crate::daemon::protocol::negotiate_pane_rendering(&mut stream, true).unwrap();
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

/// The actual Ubuntu `libsixel-bin 1.10.5-1build1` output of
/// `img2sixel -w 40 -h 40 -p 4 -d none colors.ppm` for the 2x2 red/green over
/// blue/yellow PPM. libsixel quantizes the scaled image, so the first pixel is
/// `[184, 51, 61]` in RGBA (BGRA after the production decoder).
const SIXEL_PRODUCER_OUTPUT: &[u8] = b"\x1bPq\"1;1;40;40#0;2;72;20;24#1;2;24;74;11#2;2;74;74;24#3;2;24;20;66#0!19~#1!21~-#0!19~_$#1!19?^!20~-#0!20~#1^!19~$#2!20?_-#0!13BFFN^~~~#2~}!6{!12w$#3!13{wwo_#1!4?@!6B!12F-#3!18~}w#2!20~$#0!18?@F-#3!20~#2!20~-#3!20N#2!20N\x1b\\";

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
    graphics_wait_fixture(view, cx, serial, input_bytes);
}

fn graphics_wait_fixture(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    input_bytes: usize,
) {
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
            // The fixture daemon is this same build and the stream above was
            // negotiated with it, so its version describes the real endpoint.
            let terminal = RemoteTerminal::from_stream_with_endpoint(
                stream,
                size,
                buffered,
                source,
                Some(crate::daemon::protocol::DaemonVersion::current()),
            )
            .unwrap();
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

fn graphics_window_geometry(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    size: TermSize,
) -> (gpui_kit::Size<Pixels>, usize, usize, f32) {
    use gpui_kit::{Hsla, SharedString, TextRun};
    window
        .update(cx, |_, window, cx| {
            let view = view.read(cx);
            let sample = window.text_system().shape_line(
                SharedString::new_static("M"),
                view.font_size,
                &[TextRun {
                    len: 1,
                    font: view.font.clone(),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let width = sample.width.max(px(1.));
            let height = px((view.font_size.as_f32() * view.line_height_mul).round()).max(px(1.));
            (
                gpui_kit::size(
                    width * size.cols as f32 + px(2. * GRID_PAD_X + 0.5),
                    height * size.rows as f32 + px(2. * GRID_PAD_Y + 0.5),
                ),
                size.cols,
                size.rows,
                window.scale_factor(),
            )
        })
        .unwrap()
}

fn graphics_consumer_progress(cx: &mut TestAppContext) {
    cx.update(|cx| cx.refresh_windows());
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(5));
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(5));
}

fn graphics_recovery_wait(
    cx: &mut TestAppContext,
    reason: &str,
    ready: impl FnMut(&mut TestAppContext) -> bool,
) {
    graphics_recovery_wait_with(cx, std::time::Duration::from_secs(30), ready, |_| {
        reason.to_owned()
    });
}

/// Native agent startup and full-size replay need longer deadlines and live
/// diagnostics, but must pump the same production consumer lifecycle.
#[track_caller]
fn graphics_recovery_wait_with(
    cx: &mut TestAppContext,
    timeout: std::time::Duration,
    mut ready: impl FnMut(&mut TestAppContext) -> bool,
    mut diagnostics: impl FnMut(&mut TestAppContext) -> String,
) {
    let deadline = std::time::Instant::now() + timeout;
    while !ready(cx) {
        assert!(std::time::Instant::now() < deadline, "{}", diagnostics(cx));
        graphics_consumer_progress(cx);
    }
}

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

// Controlled real PTY -> daemon -> RemoteTerminal -> production TerminalElement.
// The buffer below is the actual completed paint input, not a live-grid capture.
fn reading_painted_rows(view: &Entity<TerminalView>, cx: &mut TestAppContext) -> Vec<String> {
    view.read_with(cx, |view, _| {
        let rows = view.cell_geom.expect("production frame").rows;
        (0..rows)
            .map(|row| sync_presented_row(view, row).trim_end().to_owned())
            .collect()
    })
}

fn graphics_visible_images(view: &TerminalView) -> Vec<crate::terminal::images::RenderPlacement> {
    use lyrel_core::core::kitty_graphics::placeholder_row;
    let term = view.terminal.term.lock();
    let offset = term.display_offset() as i32;
    let rows: Vec<_> = (0..term.screen_lines())
        .filter_map(|row| {
            let slices = placeholder_row(
                &term.grid,
                rio_vt::crosswords::pos::Line(row as i32 - offset),
            );
            (!slices.is_empty()).then_some((row as i32, slices))
        })
        .collect();
    view.terminal.images().snapshot_with_placeholders(&rows)
}
fn graphics_assert_production_paint(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
) {
    let size = view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        TermSize::new(term.columns(), term.screen_lines())
    });
    graphics_assert_production_paint_at(window, view, cx, size);
}

fn graphics_assert_production_paint_at(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    size: TermSize,
) {
    use gpui_kit::test::TestWindowExt as _;
    use std::sync::atomic::Ordering;
    let (target, cols, rows, scale) = graphics_window_geometry(window, view, cx, size);
    // Preserve the requested grid through the actual prepaint sizing callback;
    // otherwise a default test window would silently resize the real daemon.
    cx.simulate_window_resize(window, target);
    let previously_painted = view.update(cx, |view, _| {
        let mut images = view
            .grid_snap
            .as_ref()
            .map(|snap| snap.images.to_vec())
            .unwrap_or_default();
        // Reset both sides: frozen reading owns old leases, while a newly
        // decoded live resource may not have entered the previous frame yet.
        images.extend(graphics_visible_images(view));
        let previous: Vec<_> = images
            .iter()
            .map(|image| (image.painted.clone(), image.painted.load(Ordering::Acquire)))
            .collect();
        for image in images {
            // An evicted placement cannot be painted again; keep its last
            // claim instead of clearing a flag no later frame can restore.
            if image.evicted {
                continue;
            }
            image.painted.store(false, Ordering::Release);
        }
        previous
    });
    window
        .update(cx, |_, window, _| {
            window.activate_window();
            window.refresh();
        })
        .unwrap();
    // Decoded resources are live parser state, not a committed presentation.
    // In particular, OpenCode can expose all six resources inside BSU before
    // ESU (or an ordered resize) permits TerminalElement to replace its frame.
    // Pump the real reader/decoder/window lifecycle instead of advancing only
    // GPUI's virtual clock and comparing the new resources to a held picture.
    graphics_recovery_wait_with(
        cx,
        std::time::Duration::from_secs(30),
        |cx| {
            window
                .update(cx, |_, window, cx| window.render_frame(cx))
                .unwrap();
            view.read_with(cx, |view, _| {
                let Some(geom) = view.cell_geom else {
                    return false;
                };
                let Some(snap) = view.grid_snap.as_ref() else {
                    return false;
                };
                if (geom.cols, geom.rows) != (cols, rows) {
                    return false;
                }
                // Reading mode intentionally presents a frozen picture, rather
                // than the advancing live image map. An ordered resize must first
                // publish its relocated cells and graphics leases atomically.
                if let Some(reading) = view.selection_scroll.reading.as_ref() {
                    return !reading.relocating();
                }
                let term = view.terminal.term.lock();
                if snap.held || term.synchronized_output() {
                    return false;
                }
                if snap.display_offset != term.display_offset() as i32 {
                    return false;
                }
                drop(term);
                let images = graphics_visible_images(view);
                images.len() == snap.images.len()
                    && images
                        .iter()
                        .zip(snap.images.iter())
                        .all(|(live, presented)| {
                            live.placement == presented.placement
                                && std::sync::Arc::ptr_eq(&live.painted, &presented.painted)
                                && match (&live.data, &presented.data) {
                                    (Some(live), Some(presented)) => {
                                        std::sync::Arc::ptr_eq(live, presented)
                                    }
                                    (None, None) => true,
                                    _ => false,
                                }
                                && live.paint_bounds(
                                    geom.origin,
                                    geom.cell_width,
                                    geom.line_height,
                                    0,
                                    snap.display_offset,
                                    scale,
                                ) == presented.paint_bounds(
                                    geom.origin,
                                    geom.cell_width,
                                    geom.line_height,
                                    0,
                                    snap.display_offset,
                                    scale,
                                )
                        })
            })
        },
        |cx| {
            view.read_with(cx, |view, _| format!(
            "production presentation did not commit current {cols}x{rows} resources; held={:?}; synchronized={}; graphics={}",
            view.grid_snap.as_ref().map(|snap| snap.held),
            view.terminal.term.lock().synchronized_output(),
            view.terminal.images().recovery_diagnostics(),
        ))
        },
    );
    window.update(cx, |_, window, cx| {
        let surface = window.find("terminal-surface").bounds();
        let view = view.read(cx);
        let geom = view.cell_geom.expect("rooted production TerminalElement must paint a fresh frame");
        assert_eq!((geom.cols, geom.rows), (cols, rows));
        // Inspect the picture actually consumed by production paint, not a
        // fresh projection of live placements under frozen reading/held text.
        let snap = view.grid_snap.as_ref().expect("committed production frame");
        // Observe the stationary pane in GPUI's completed frame, independently
        // of the shifted cell-row origin or the snapshot's private scroll state.
        // The preceding row's image can therefore occupy its top sliver.
        let viewport = Bounds::new(
            surface.origin + point(px(GRID_PAD_X), px(GRID_PAD_Y)),
            gpui_kit::size(geom.cell_width * cols as f32, geom.line_height * rows as f32),
        );
        let offset = snap.display_offset;
        let images = &snap.images;
        for image in images.iter() {
            // An evicted placement is historical: it cannot be painted in
            // this frame, and its last claim is intentionally preserved.
            if image.evicted {
                continue;
            }
            let expected = images.iter().filter(|other| std::sync::Arc::ptr_eq(&image.painted, &other.painted)).any(|other| {
                let (crop, _) = other.paint_bounds(geom.origin, geom.cell_width, geom.line_height, 0, offset, scale);
                let visible = crop.intersect(&viewport);
                visible.size.width > px(0.) && visible.size.height > px(0.)
            });
            assert_eq!(image.painted.load(Ordering::Acquire), expected, "fresh production paint claim must match the current visible resource geometry; not GPU pixels");
        }
    }).unwrap();
    // The flag also tracks atlas ownership. An offscreen texture painted in a
    // previous frame must still be retired when a later user action removes it.
    for (painted, previous) in previously_painted {
        if previous {
            painted.store(true, Ordering::Release);
        }
    }
}
fn graphics_recovery_destroy(
    view: Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    #[cfg(test)]
    let entity_id = view.entity_id();
    let weak_view = view.downgrade();
    let (store_released, pixels) = view.read_with(cx, |view, _| {
        let store = view.terminal.images();
        let pixels: Vec<_> = store
            .snapshot()
            .iter()
            .filter_map(|image| image.data.as_ref().map(std::sync::Arc::downgrade))
            .collect();
        (store.release_probe(), pixels)
    });
    // The headless platform keeps its active TestWindow (and that window's
    // production IME handler, which owns a strong TerminalView) independently
    // of App's window map. Simulate the native blur before removing the window
    // so the test platform cannot keep the closed consumer alive forever.
    gpui_kit::VisualTestContext::from_window(window, cx).deactivate_window();
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    drop(view);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // Dropping the final Entity handle queues its value for release. The
        // normal App::update turn flushes that queue and invokes on_release;
        // running the test dispatcher alone does not drop the entity value.
        cx.update(|_| {});
        let view_alive = weak_view.upgrade().is_some();
        let store_alive = !store_released();
        let textures_alive = pixels
            .iter()
            .filter(|pixel| pixel.upgrade().is_some())
            .count();
        if !view_alive && !store_alive && textures_alive == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "old consumer did not actually die: view_alive={view_alive}, store_alive={store_alive}, textures_alive={textures_alive}"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(5));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    #[cfg(test)]
    cx.update(|cx| {
        assert_eq!(
            crate::terminal::lifecycle::displayed_for_test(cx, entity_id),
            None
        );
    });
}

// ---- Glyph Protocol producer and production-paint helpers ----------------
//
// Shared by the headless consumer tracer and the native-font consumer so both
// drive the same real PTY fixture, the same reply channel and the same
// production scene assertions.

use gpui_kit::test::TestWindowExt as _;

/// A supplementary-plane PUA-B codepoint no ordinary font maps, so a painted
/// glyph can only come from the registration.
const GLYPH_CP: u32 = 0x100000;

/// A closed simple-glyph square with four on-curve points and no hinting.
fn glyph_square(side: i16) -> Vec<u8> {
    glyph_polygon(&[(0, 0), (side, 0), (side, side), (0, side)])
}

/// A closed simple-glyph rectangle.
#[allow(dead_code)]
fn glyph_rect(width: i16, height: i16) -> Vec<u8> {
    glyph_polygon(&[(0, 0), (width, 0), (width, height), (0, height)])
}

fn glyph_polygon(points: &[(i16, i16)]) -> Vec<u8> {
    let count = points.len() as u16;
    let xs: Vec<i16> = points.iter().map(|(x, _)| *x).collect();
    let ys: Vec<i16> = points.iter().map(|(_, y)| *y).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&1i16.to_be_bytes()); // one contour
    out.extend_from_slice(&xs.iter().copied().min().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&ys.iter().copied().min().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&xs.iter().copied().max().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&ys.iter().copied().max().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&(count - 1).to_be_bytes()); // endPtsOfContours
    out.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
    out.extend(std::iter::repeat_n(0x01u8, points.len())); // on-curve, 16-bit deltas
    let mut previous = 0i16;
    for x in &xs {
        out.extend_from_slice(&(x - previous).to_be_bytes());
        previous = *x;
    }
    let mut previous = 0i16;
    for y in &ys {
        out.extend_from_slice(&(y - previous).to_be_bytes());
        previous = *y;
    }
    out
}

#[allow(dead_code)]
fn glyph_register(cp: u32, glyf: &[u8], reply: u8) -> Vec<u8> {
    glyph_register_with(cp, glyf, 1000, "", reply)
}

/// A registration with the §8.5 control keys the caller writes verbatim.
/// `params` is a semicolon-separated list without a trailing separator.
#[allow(dead_code)]
fn glyph_register_with(cp: u32, glyf: &[u8], upm: u16, params: &str, reply: u8) -> Vec<u8> {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(glyf);
    let params = params.trim_end_matches(';');
    let separator = if params.is_empty() { "" } else { ";" };
    format!(
        "\x1b_25a1;r;cp={cp:x};fmt=glyf;reply={reply};upm={upm};{params}{separator}{payload}\x1b\\"
    )
    .into_bytes()
}

/// Send one registration through the real PTY and require the daemon's
/// success reply.
#[allow(dead_code)]
fn glyph_register_ok(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    reply_path: &std::path::Path,
    cp: u32,
    glyf: &[u8],
    upm: u16,
    params: &str,
) {
    let reply = glyph_send(
        view,
        cx,
        serial,
        reply_path,
        &glyph_register_with(cp, glyf, upm, params, 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={cp:x};status=0\x1b\\").into_bytes(),
        "registration rejected: {params}"
    );
}

/// Place text at a zero-based row/column through the real PTY fixture.
#[allow(dead_code)]
fn glyph_show(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    row: usize,
    col: usize,
    text: &str,
) {
    let bytes = format!("\x1b[{};{}H{text}", row + 1, col + 1).into_bytes();
    graphics_send_fixture(view, cx, serial, &bytes);
}

fn glyph_query(cp: u32) -> Vec<u8> {
    format!("\x1b_25a1;q;cp={cp:x}\x1b\\").into_bytes()
}

#[allow(dead_code)]
fn glyph_clear(cp: Option<u32>) -> Vec<u8> {
    match cp {
        Some(cp) => format!("\x1b_25a1;c;cp={cp:x}\x1b\\").into_bytes(),
        None => b"\x1b_25a1;c\x1b\\".to_vec(),
    }
}

/// Send through the real PTY fixture and read `replies` back through the
/// request's gate file: Glyph Protocol has its own reply vocabulary, and a
/// row-1 reply line would be overwritten by the next request.
fn glyph_send(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    reply_path: &std::path::Path,
    bytes: &[u8],
    replies: usize,
) -> Vec<u8> {
    let path = reply_path.to_string_lossy();
    let request = format!(
        "SEND_ANY {serial} {replies} {} 0 {}\n",
        graphics_pty_fixture::hex(bytes),
        graphics_pty_fixture::hex(path.as_bytes()),
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(view, cx, serial, input_bytes);
    let text = std::fs::read_to_string(reply_path)
        .unwrap_or_else(|error| panic!("no reply file for serial {serial}: {error}"));
    graphics_pty_fixture::unhex(text.trim())
}

fn glyph_cell_bounds(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    row: usize,
    col: usize,
) -> gpui_kit::Bounds<gpui_kit::ScaledPixels> {
    glyph_region_bounds(window, view, cx, row, col, 1)
}

/// The declared render span of a registration, in the scaled pixels the
/// painted scene records. `cells` is the span the production code chose, so
/// a width-two registration that fell back to one cell asks for one here.
#[allow(dead_code)]
fn glyph_region_bounds(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    row: usize,
    col: usize,
    cells: usize,
) -> gpui_kit::Bounds<gpui_kit::ScaledPixels> {
    let geom = view.read_with(cx, |view, _| view.cell_geom.expect("production frame"));
    let scale = window
        .update(cx, |_, window, _| window.scale_factor())
        .unwrap();
    gpui_kit::Bounds::new(
        gpui_kit::point(
            geom.origin.x + geom.cell_width * col as f32,
            geom.origin.y + geom.line_height * row as f32,
        ),
        gpui_kit::size(geom.cell_width * cells as f32, geom.line_height),
    )
    .map(|pixel| gpui_kit::ScaledPixels(pixel.as_f32() * scale))
}

/// Whether two painted rectangles land on the same pixels within the
/// fractional layout and tessellation tolerance of one physical pixel.
#[allow(dead_code)]
fn glyph_bounds_close(
    left: gpui_kit::Bounds<gpui_kit::ScaledPixels>,
    right: gpui_kit::Bounds<gpui_kit::ScaledPixels>,
) -> bool {
    let close = |a: f32, b: f32| (a - b).abs() <= 1.5;
    close(left.origin.x.0, right.origin.x.0)
        && close(left.origin.y.0, right.origin.y.0)
        && close(left.size.width.0, right.size.width.0)
        && close(left.size.height.0, right.size.height.0)
}

/// Renders one production frame and returns the path primitives that land
/// inside `cell`, with the solid colour they were painted with.
fn glyph_paths_in_cell(
    window: gpui_kit::AnyWindowHandle,
    _view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    cell: gpui_kit::Bounds<gpui_kit::ScaledPixels>,
) -> Vec<(
    gpui_kit::Bounds<gpui_kit::ScaledPixels>,
    gpui_kit::Background,
)> {
    window
        .update(cx, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
            window
                .painted_scene()
                .paths
                .iter()
                .filter(|path| {
                    let visible = path.bounds.intersect(&cell);
                    visible.size.width.0 > 0. && visible.size.height.0 > 0.
                })
                .map(|path| (path.bounds, path.color))
                .collect()
        })
        .unwrap()
}

/// The same production paths, with the content mask that the renderer clips
/// them to. A glyph's declared span is that mask, so a test can prove the
/// visible extent even when the path geometry itself runs past it.
#[allow(dead_code)]
fn glyph_paths_with_clip_in_region(
    window: gpui_kit::AnyWindowHandle,
    _view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    region: gpui_kit::Bounds<gpui_kit::ScaledPixels>,
) -> Vec<(
    gpui_kit::Bounds<gpui_kit::ScaledPixels>,
    gpui_kit::Bounds<gpui_kit::ScaledPixels>,
    gpui_kit::Background,
)> {
    window
        .update(cx, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
            window
                .painted_scene()
                .paths
                .iter()
                .filter(|path| {
                    let visible = path.bounds.intersect(&region);
                    visible.size.width.0 > 0. && visible.size.height.0 > 0.
                })
                .map(|path| (path.bounds, path.content_mask.bounds, path.color))
                .collect()
        })
        .unwrap()
}

fn glyph_wait(
    cx: &mut TestAppContext,
    what: &str,
    mut ready: impl FnMut(&mut TestAppContext) -> bool,
) {
    graphics_recovery_wait(cx, what, |cx| ready(cx));
}

fn glyph_presented_row(view: &TerminalView, row: usize) -> String {
    let cols = view.cell_geom.expect("a production frame was painted").cols;
    view.grid_buf[row * cols..(row + 1) * cols]
        .iter()
        .map(|cell| if cell.c == '\0' { ' ' } else { cell.c })
        .collect()
}
