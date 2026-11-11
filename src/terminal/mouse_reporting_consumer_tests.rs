// The observer is a real raw-mode PTY child. Expected reports are asserted at
// its stdin, after dispatch through rooted production TerminalElement handlers.
#[test]
#[ignore = "raw PTY child for mouse ownership consumer tests"]
fn mouse_reporting_fixture_recorder() {
    use std::io::{Read as _, Write as _};
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    graphics_pty_fixture::raw_pty();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output
        .write_all(b"\x1b[?1002h\x1b[?1006h\r\nMOUSE_READY\r\n")
        .unwrap();
    output.flush().unwrap();
    let mut report = Vec::new();
    let mut serial = 0;
    loop {
        let mut byte = [0];
        if input.read(&mut byte).unwrap() == 0 {
            return;
        }
        match byte[0] {
            4 => {
                serial += 1;
                let path = format!("mouse-{serial}.bin");
                std::fs::write("mouse-pending.bin", &report).unwrap();
                std::fs::rename("mouse-pending.bin", path).unwrap();
                report.clear();
            }
            5 => {
                output.write_all(b"\x1b[?1002l\x1b[?1006l").unwrap();
                output.flush().unwrap();
            }
            6 => {
                output.write_all(b"\x1b[?1002h\x1b[?1006h").unwrap();
                output.flush().unwrap();
            }
            7 => {
                for row in 0..72 {
                    write!(output, "WHEEL-{row:03}\r\n").unwrap();
                }
                output.write_all(b"\x1b[8;1HMOUSE-HISTORY-READY").unwrap();
                output.flush().unwrap();
            }
            8 => {
                output.write_all(b"\x1b[?1002l\x1b[?1006l\x1b[?1049h\x1b[?1007h\x1b[?1h\x1b[HALT-WHEEL-READY").unwrap();
                output.flush().unwrap();
            }
            9 => {
                output.write_all(b"\x1b[?1049l\x1b[?1l\x1b[8;1HNORMAL-WHEEL-READY").unwrap();
                output.flush().unwrap();
            }
            byte => report.push(byte),
        }
    }
}

impl GraphicsDaemonProcess {
    fn spawn_mouse_recorder(&self, name: &str) -> (u64, std::path::PathBuf) {
        use crate::daemon::protocol::{ShellSpec, WinSize};
        let dir = self.dir.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        let shell = ShellSpec {
            program: std::env::current_exe().unwrap().to_str().unwrap().into(),
            args: vec![
                "--ignored".into(),
                "--exact".into(),
                format!(
                    "{}::mouse_reporting_fixture_recorder",
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
                Some(dir.clone()),
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
            .windows(b"MOUSE_READY".len())
            .any(|part| part == b"MOUSE_READY")
        {
            match session.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => text.extend(bytes),
                DaemonMsg::Error(error) => panic!("mouse PTY fixture failed: {error}"),
                DaemonMsg::Exited { code } => panic!("mouse PTY fixture exited: {code:?}"),
                _ => {}
            }
        }
        let pane = session.pane_id();
        session.detach().unwrap();
        (pane, dir)
    }
}

struct MouseReportingSplit {
    panes: [Entity<TerminalView>; 2],
}

impl Render for MouseReportingSplit {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(div().w(px(200.)).h_full())
            .children(
                self.panes
                    .iter()
                    .map(|view| div().flex_1().h_full().child(view.clone())),
            )
    }
}

fn mouse_reporting_event(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    event: gpui_kit::PlatformInput,
) {
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(event, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn mouse_reporting_move(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    position: gpui_kit::Point<Pixels>,
    pressed_button: Option<MouseButton>,
    modifiers: Modifiers,
) {
    mouse_reporting_event(
        cx,
        window,
        gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
            position,
            pressed_button,
            modifiers,
        }),
    );
}

fn mouse_reporting_button(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    position: gpui_kit::Point<Pixels>,
    down: bool,
    modifiers: Modifiers,
) {
    mouse_reporting_button_for(cx, window, position, MouseButton::Left, down, modifiers);
}

fn mouse_reporting_button_for(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    position: gpui_kit::Point<Pixels>,
    button: MouseButton,
    down: bool,
    modifiers: Modifiers,
) {
    let event = if down {
        gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        })
    } else {
        gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
            button,
            position,
            modifiers,
            click_count: 1,
        })
    };
    mouse_reporting_event(cx, window, event);
}

fn mouse_reporting_checkpoint(
    cx: &mut TestAppContext,
    panes: &[Entity<TerminalView>; 2],
    dirs: &[std::path::PathBuf; 2],
    serial: usize,
) -> [Vec<u8>; 2] {
    for view in panes {
        view.read_with(cx, |view, _| view.terminal.write(vec![4]));
    }
    let paths = dirs
        .each_ref()
        .map(|dir| dir.join(format!("mouse-{serial}.bin")));
    graphics_recovery_wait(cx, "mouse PTY input barrier did not arrive", |_| {
        paths.iter().all(|path| path.exists())
    });
    let reports = paths.map(|path| std::fs::read(path).unwrap());
    eprintln!(
        "mouse PTY checkpoint {serial}: {:?}",
        reports
            .each_ref()
            .map(|bytes| String::from_utf8_lossy(bytes))
    );
    reports
}

#[gpui_kit::test]
fn mouse_reporting_only_the_pressed_pane_owns_real_pty_reports(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let [(a_id, a_dir), (b_id, b_dir)] = [
        daemon.spawn_mouse_recorder("a"),
        daemon.spawn_mouse_recorder("b"),
    ];
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let window: gpui_kit::AnyWindowHandle = cx.add_window(|_, _| gpui_kit::EmptyView).into();
    let panes = window
        .update(cx, |_, window, cx| {
            [a_id, b_id].map(|pane| {
                let source = if cfg!(windows) {
                    PtySource::LocalConpty
                } else {
                    PtySource::Raw
                };
                let terminal = RemoteTerminal::from_stream_with(
                    daemon.stream(pane),
                    TermSize::new(80, 24),
                    Vec::new(),
                    source,
                )
                .unwrap();
                cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx))
            })
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                let split = cx.new(|_| MouseReportingSplit {
                    panes: panes.clone(),
                });
                gpui_kit::component::Root::new(split, window, cx)
            });
        })
        .unwrap();
    graphics_recovery_wait(cx, "both real PTY consumers must paint mouse mode", |cx| {
        panes
            .iter()
            .all(|view| view.read_with(cx, |view, _| view.mouse_mode() && view.cell_geom.is_some()))
    });
    let [ga, gb] = panes
        .each_ref()
        .map(|view| view.read_with(cx, |view, _| view.cell_geom.unwrap()));
    assert!(ga.origin.x >= px(200.) && gb.origin.x > ga.origin.x);
    let at = |g: super::super::element::CellGeom, col: f32, row: f32| {
        point(
            g.origin.x + g.cell_width * (col + 0.5),
            g.origin.y + g.line_height * (row + 0.5),
        )
    };
    let dirs = [a_dir, b_dir];
    let plain = Modifiers::default();
    // A title/sidebar gesture never belongs to either terminal.
    let side = point(px(50.), px(100.));
    mouse_reporting_move(cx, window, side, None, plain);
    mouse_reporting_button(cx, window, side, true, plain);
    mouse_reporting_move(
        cx,
        window,
        point(px(50.), px(180.)),
        Some(MouseButton::Left),
        plain,
    );
    mouse_reporting_button(cx, window, side, false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 1),
        [Vec::<u8>::new(), Vec::new()],
        "no pane reported this press"
    );

    // A reported press keeps its clamped drag even across the neighbouring pane.
    mouse_reporting_move(cx, window, at(ga, 2., 3.), None, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), true, plain);
    mouse_reporting_move(cx, window, at(ga, 4., 3.), Some(MouseButton::Left), plain);
    mouse_reporting_move(cx, window, at(gb, 1., 5.), Some(MouseButton::Left), plain);
    mouse_reporting_button(cx, window, at(gb, 1., 5.), false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 2),
        [
            format!(
                "\x1b[<0;3;4M\x1b[<32;5;4M\x1b[<32;{};6M\x1b[<0;{};6m",
                ga.cols, ga.cols
            )
            .into_bytes(),
            Vec::new(),
        ]
    );

    // Shift selection remains local even if Shift is released mid-gesture.
    let shift = Modifiers {
        shift: true,
        ..plain
    };
    mouse_reporting_move(cx, window, at(ga, 2., 3.), None, shift);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), true, shift);
    mouse_reporting_move(cx, window, at(ga, 4., 3.), Some(MouseButton::Left), plain);
    mouse_reporting_button(cx, window, at(ga, 4., 3.), false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 3),
        [Vec::<u8>::new(), Vec::new()]
    );

    // Releasing with Shift still retires the grab; it cannot own another drag.
    mouse_reporting_move(cx, window, at(ga, 2., 3.), None, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), true, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), false, shift);
    mouse_reporting_move(cx, window, at(ga, 4., 3.), Some(MouseButton::Left), plain);
    mouse_reporting_button(cx, window, at(ga, 4., 3.), false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 4),
        [b"\x1b[<0;3;4M".to_vec(), Vec::new()]
    );

    // A release lost to another window is retired by the next buttonless move.
    mouse_reporting_move(cx, window, at(ga, 2., 3.), None, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), true, plain);
    mouse_reporting_move(cx, window, side, None, plain);
    mouse_reporting_move(cx, window, at(ga, 4., 3.), Some(MouseButton::Left), plain);
    mouse_reporting_button(cx, window, at(ga, 4., 3.), false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 5),
        [b"\x1b[<0;3;4M".to_vec(), Vec::new()]
    );

    // Do not solve the defect by disabling legitimate click or wheel reports.
    mouse_reporting_move(cx, window, at(ga, 2., 3.), None, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), true, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), false, plain);
    mouse_reporting_event(
        cx,
        window,
        gpui_kit::PlatformInput::Scroll(ScrollEvent {
            position: at(ga, 2., 3.),
            delta: gpui_kit::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        }),
    );
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 6),
        [
            b"\x1b[<0;3;4M\x1b[<0;3;4m\x1b[<64;3;4M".to_vec(),
            Vec::new()
        ]
    );

    // Enabling reporting during a local gesture does not manufacture a press.
    panes[0].read_with(cx, |view, _| view.terminal.write(vec![5]));
    graphics_recovery_wait(cx, "the PTY did not disable reporting", |cx| {
        panes[0].read_with(cx, |view, _| !view.mouse_mode())
    });
    mouse_reporting_move(cx, window, at(ga, 2., 3.), None, plain);
    mouse_reporting_button(cx, window, at(ga, 2., 3.), true, plain);
    panes[0].read_with(cx, |view, _| view.terminal.write(vec![6]));
    graphics_recovery_wait(cx, "the PTY did not enable reporting", |cx| {
        panes[0].read_with(cx, |view, _| view.mouse_mode())
    });
    mouse_reporting_move(cx, window, at(ga, 4., 3.), Some(MouseButton::Left), plain);
    mouse_reporting_button(cx, window, at(ga, 4., 3.), false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 7),
        [Vec::<u8>::new(), Vec::new()]
    );

    // Every reported button owns its release independently of other buttons.
    for (offset, first, second, expected) in [
        (
            0,
            MouseButton::Left,
            MouseButton::Right,
            b"\x1b[<0;3;4M\x1b[<2;3;4M\x1b[<2;3;4m\x1b[<0;3;4m",
        ),
        (
            1,
            MouseButton::Middle,
            MouseButton::Left,
            b"\x1b[<1;3;4M\x1b[<0;3;4M\x1b[<0;3;4m\x1b[<1;3;4m",
        ),
        (
            2,
            MouseButton::Right,
            MouseButton::Middle,
            b"\x1b[<2;3;4M\x1b[<1;3;4M\x1b[<1;3;4m\x1b[<2;3;4m",
        ),
    ] {
        let position = at(ga, 2., 3.);
        mouse_reporting_move(cx, window, position, None, plain);
        mouse_reporting_button_for(cx, window, position, first, true, plain);
        mouse_reporting_button_for(cx, window, position, second, true, plain);
        mouse_reporting_button_for(cx, window, position, second, false, plain);
        mouse_reporting_button_for(cx, window, position, first, false, plain);
        assert_eq!(
            mouse_reporting_checkpoint(cx, &panes, &dirs, 8 + offset),
            [expected.to_vec(), Vec::new()]
        );
    }

    // A buttonless move retires all held owners, not merely the last press.
    let position = at(ga, 2., 3.);
    mouse_reporting_button_for(cx, window, position, MouseButton::Left, true, plain);
    mouse_reporting_button_for(cx, window, position, MouseButton::Right, true, plain);
    mouse_reporting_move(cx, window, side, None, plain);
    mouse_reporting_button_for(cx, window, position, MouseButton::Right, false, plain);
    mouse_reporting_button_for(cx, window, position, MouseButton::Left, false, plain);
    assert_eq!(
        mouse_reporting_checkpoint(cx, &panes, &dirs, 11),
        [b"\x1b[<0;3;4M\x1b[<2;3;4M".to_vec(), Vec::new()]
    );
}
