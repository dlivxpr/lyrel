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
    graphics_recovery_wait_with(cx, std::time::Duration::from_secs(30), ready, |_| reason.to_owned());
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
        .flat_map(|cell| std::iter::once(cell.c).chain(cell.marks.as_deref().unwrap_or(&[]).iter().copied()))
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
