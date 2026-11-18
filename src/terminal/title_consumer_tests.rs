// Consumer-visible title and OSC 8 contracts through the production window,
// the real isolated daemon/PTY, and the ordered pane stream.

use std::time::Duration;

use lyrel_core::client::ControlClient;
use lyrel_core::daemon::control::{ControlHello, ControlRequest, ReplyOk};
use lyrel_core::daemon::protocol::{DaemonVersion, PROTOCOL_VERSION};

use super::notices::TITLE_SETTLE;

fn title_control(daemon: &GraphicsDaemonProcess) -> ControlClient {
    ControlClient::connect_at(
        &daemon.endpoint("control"),
        &ControlHello::host_rpc("title-consumer", "title-consumer"),
    )
    .expect("the isolated daemon serves control")
}

/// The daemon's authoritative title fact, which every control consumer — the
/// sidebar, the switcher, and the CLI's tree — reads.
fn daemon_title(control: &ControlClient, pane: u64) -> Option<String> {
    let ReplyOk::MachineTree(machine) = control
        .request(ControlRequest::MachineGet)
        .expect("MachineGet answers")
    else {
        panic!("MachineGet answered with something else");
    };
    machine
        .panes
        .iter()
        .find(|record| record.id == pane)
        .and_then(|record| record.osc_title.clone())
}

fn wait_daemon_title(control: &ControlClient, pane: u64, want: Option<&str>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let got = daemon_title(control, pane);
        if got.as_deref() == want {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "daemon title never became {want:?}; last was {got:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sends title bytes through the fixture's real PTY and waits for the
/// plain-text certificate it writes once the daemon has consumed the request.
/// Its `TITLE` operation never writes the shared `KITTY_DONE` marker, whose
/// own OSC 2 would replace the title under test.
fn send_title_fixture(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
) {
    let request = format!("TITLE {serial} 0 {} 0\n", graphics_pty_fixture::hex(bytes));
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let marker = format!("TITLE_DONE_{serial}");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| view.scrollback_text().contains(&marker)) {
            return;
        }
        // The fixture runs its own barrier assertion; a dead producer must be
        // reported with its own output instead of burning the whole deadline.
        if view.read_with(cx, |view, _| view.terminal.child_exited()) {
            view.read_with(cx, |view, _| {
                let text = view.scrollback_text();
                let start = text
                    .char_indices()
                    .rev()
                    .nth(255)
                    .map_or(0, |(index, _)| index);
                panic!(
                    "the PTY title fixture exited before completing request {serial}; tail={:?}",
                    &text[start..]
                );
            });
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the PTY title fixture never completed request {serial}"
        );
        graphics_consumer_progress(cx);
    }
}

/// Runs out the tab's title-settle timer and waits for the visible title.
fn settle_window_title(view: &Entity<TerminalView>, cx: &mut TestAppContext, want: &str) {
    for _ in 0..400 {
        cx.run_until_parked();
        cx.executor().advance_clock(TITLE_SETTLE * 2);
        cx.run_until_parked();
        if view.read_with(cx, |view, _| view.title == want) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!(
        "window title never became {want:?}; last was {:?}",
        view.read_with(cx, |view, _| view.title.clone())
    );
}

fn pty_source() -> PtySource {
    if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    }
}

/// The real PTY producer, the isolated production daemon, and the rooted
/// window consumer. Every title rule is checked against both the daemon's
/// authoritative fact and the window, including a fresh attach replay and a
/// pane hidden at the time the title arrives.
#[gpui_kit::test]
fn title_consumer_real_pty_daemon_and_window_agree_and_reattach_replays(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        let mut config = Config::default();
        config.cursor_blink = false;
        cx.set_global(config);
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, _window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let control = title_control(&daemon);

    // A hidden pane still consumes the title for its tab label; visibility
    // gates painting, not the ordered facts. The registry reports the pane as
    // hidden, so the title below really arrives while it is not on screen.
    cx.update(|cx| {
        declare_displayed(cx, [(view.entity_id(), false)]);
        assert_eq!(
            crate::terminal::lifecycle::displayed_for_test(cx, view.entity_id()),
            Some(false),
            "the pane consumer is actually hidden for this title"
        );
    });
    send_title_fixture(&view, cx, 1, b"\x1b]2;  hidden ; title  \x1b\\");
    settle_window_title(&view, cx, "  hidden ; title  ");
    wait_daemon_title(&control, pane, Some("  hidden ; title  "));

    // 255 UTF-8 bytes are accepted whole, spaces and semicolons included.
    let at_limit = format!("  {} ;end ", "t".repeat(247));
    assert_eq!(at_limit.len(), 255);
    send_title_fixture(&view, cx, 2, format!("\x1b]0;{at_limit}\x1b\\").as_bytes());
    settle_window_title(&view, cx, &at_limit);
    wait_daemon_title(&control, pane, Some(&at_limit));

    // 256 bytes are rejected whole; neither consumer truncates or loses the
    // previous valid title.
    let over = "x".repeat(256);
    send_title_fixture(&view, cx, 3, format!("\x1b]2;{over}\x07").as_bytes());
    settle_window_title(&view, cx, &at_limit);
    wait_daemon_title(&control, pane, Some(&at_limit));

    // Multibyte boundary: 85 CJK characters are exactly 255 bytes; one more
    // is rejected.
    let cjk = "中".repeat(85);
    assert_eq!(cjk.len(), 255);
    send_title_fixture(&view, cx, 4, format!("\x1b]2;{cjk}\x1b\\").as_bytes());
    settle_window_title(&view, cx, &cjk);
    wait_daemon_title(&control, pane, Some(&cjk));
    send_title_fixture(
        &view,
        cx,
        5,
        format!("\x1b]2;{}\x1b\\", "中".repeat(86)).as_bytes(),
    );
    settle_window_title(&view, cx, &cjk);
    wait_daemon_title(&control, pane, Some(&cjk));

    // Invalid UTF-8 is rejected whole. ConPTY may rewrite arbitrary bytes
    // before the daemon sees them, so the real-PTY byte path is pinned where
    // the PTY is byte-exact; the ordered-stream consumer below covers the same
    // payload on every platform.
    #[cfg(unix)]
    {
        send_title_fixture(&view, cx, 6, b"\x1b]2;bad \xff utf8\x07");
        settle_window_title(&view, cx, &cjk);
        wait_daemon_title(&control, pane, Some(&cjk));
    }

    // A valid empty payload is the reset, not an ignored report.
    send_title_fixture(&view, cx, 7, b"\x1b]2;\x07");
    settle_window_title(&view, cx, super::DEFAULT_TITLE);
    wait_daemon_title(&control, pane, None);

    // An accepted command title retires at its `D` on both sides.
    send_title_fixture(
        &view,
        cx,
        8,
        b"\x1b]133;C;ls\x07\x1b]2;ls -la\x07\x1b]133;D;0\x07",
    );
    settle_window_title(&view, cx, super::DEFAULT_TITLE);
    wait_daemon_title(&control, pane, None);

    // Prompt title -> command start -> rejected title -> command finish: the
    // rejected report must not mark the retained prompt title as command-owned.
    let rejected = format!("\x1b]2;{}\x07", "x".repeat(256));
    let mut scenario = Vec::new();
    scenario.extend_from_slice(b"\x1b]133;A\x07\x1b]2;prompt title\x07\x1b]133;B\x07");
    scenario.extend_from_slice(b"\x1b]133;C;claude\x07");
    scenario.extend_from_slice(rejected.as_bytes());
    scenario.extend_from_slice(b"\x1b]133;D;0\x07");
    send_title_fixture(&view, cx, 9, &scenario);
    settle_window_title(&view, cx, "prompt title");
    wait_daemon_title(&control, pane, Some("prompt title"));

    // A fresh window attached to the live pane gets the retained prompt title
    // from the daemon's ordered replay, not from anything this test kept.
    let (reattached, _window2) = graphics_build_rooted_consumer_at(
        cx,
        pane,
        daemon.stream(pane),
        Vec::new(),
        TermSize::new(80, 24),
        pty_source(),
    );
    settle_window_title(&reattached, cx, "prompt title");
}

/// An endpoint that predates the validated-title capability must keep running,
/// apply the title it can parse, and say once that its machine-side facts are
/// not guaranteed to agree.
#[gpui_kit::test]
fn title_consumer_older_endpoint_warns_once_and_keeps_the_session(cx: &mut TestAppContext) {
    use crate::ui::app::LyrelApp;
    use crate::ui::pane::{Pane, PaneSlot};
    use crate::ui::tab::Tab;

    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let (client_side, mut old_daemon) = test_stream_pair();
    let old_version = DaemonVersion {
        protocol: PROTOCOL_VERSION,
        build: "0.3.4".into(),
        features: Vec::new(),
        instance: "old-endpoint".into(),
    };
    let view = app.update_in(&mut vcx, |app, window, cx| {
        let terminal = RemoteTerminal::from_stream_with_endpoint(
            client_side,
            TermSize::new(80, 24),
            Vec::new(),
            pty_source(),
            Some(old_version),
        )
        .expect("the old endpoint still opens a pane stream");
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, 1, window, cx));
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
        app.tabs.select(0);
        cx.notify();
        view
    });

    DaemonMsg::Output(b"\x1b]2;old endpoint title  \x07".to_vec())
        .encode(&mut old_daemon)
        .unwrap();
    settle_window_title(&view, cx, "old endpoint title  ");
    assert!(
        !view.read_with(cx, |view, _| view.terminal.exited),
        "the old endpoint's session stays attached; nothing replaces it"
    );

    let messages = |vcx: &gpui_kit::VisualTestContext| {
        vcx.read(|cx| {
            crate::ui::notification_history::history(cx)
                .read(cx)
                .records()
                .map(|record| record.message().to_string())
                .collect::<Vec<_>>()
        })
    };
    vcx.run_until_parked();
    assert_eq!(
        messages(&vcx),
        vec![t(L10nKey::TerminalTitleNeedsDaemonUpdate).to_string()],
        "one honest compatibility warning for the older endpoint"
    );

    // A second title is not another warning, and it is still applied.
    DaemonMsg::Output(b"\x1b]2;second title\x07".to_vec())
        .encode(&mut old_daemon)
        .unwrap();
    settle_window_title(&view, cx, "second title");
    vcx.run_until_parked();
    assert_eq!(
        messages(&vcx).len(),
        1,
        "the warning is once per pane, not per title"
    );

    // A capable endpoint keeps the same session and says nothing: parity is
    // claimed only where the endpoint actually implements it.
    let (client_side, mut current_daemon) = test_stream_pair();
    let capable = app.update_in(&mut vcx, |app, window, cx| {
        let terminal = RemoteTerminal::from_stream_with_endpoint(
            client_side,
            TermSize::new(80, 24),
            Vec::new(),
            pty_source(),
            Some(DaemonVersion::current()),
        )
        .expect("the current endpoint opens a pane stream");
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, 2, window, cx));
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
        app.tabs.select(1);
        cx.notify();
        view
    });
    DaemonMsg::Output(b"\x1b]2;capable title\x07".to_vec())
        .encode(&mut current_daemon)
        .unwrap();
    settle_window_title(&capable, cx, "capable title");
    vcx.run_until_parked();
    assert_eq!(
        messages(&vcx).len(),
        1,
        "a capable endpoint produces no compatibility warning"
    );
}

/// The ordered pane stream is a production consumer too: invalid UTF-8 from a
/// synthetic endpoint must not become a title, and the previous valid title
/// stands on every platform (the raw byte path is PTY-exact only on Unix).
#[gpui_kit::test]
fn title_consumer_invalid_utf8_from_the_ordered_stream_is_rejected_whole(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Output(b"\x1b]2;kept title\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    let titled =
        |window: &gpui_kit::WindowHandle<TerminalView>, cx: &mut TestAppContext, want: &str| {
            for _ in 0..300 {
                cx.run_until_parked();
                cx.executor().advance_clock(TITLE_SETTLE * 2);
                cx.run_until_parked();
                if window.update(cx, |view, _, _| view.title == want).unwrap() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!(
                "window title never became {want:?}; last was {:?}",
                window.update(cx, |view, _, _| view.title.clone()).unwrap()
            );
        };
    titled(&window, cx, "kept title");
    DaemonMsg::Output(b"\x1b]2;bad \xff utf8\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    titled(&window, cx, "kept title");
}

fn hyperlink_at(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    row: usize,
    col: usize,
) -> Option<(String, String)> {
    view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        term.cell_hyperlink(
            rio_vt::crosswords::pos::Line(row as i32),
            rio_vt::crosswords::pos::Column(col),
        )
        .map(|link| (link.id().to_string(), link.uri().to_string()))
    })
}

/// The pinned Rio parser's explicit OSC 8 contract, observed in the real
/// window's cells after a real PTY/daemon round trip: semicolons stay in the
/// URI, repeated ids take the last non-empty value, a link sent without `id`
/// gets a synthesized non-empty one, 2047 combined bytes open and 2048
/// neither open nor close, an empty URI with an id is ignored, and an empty
/// URI without one closes.
#[gpui_kit::test]
fn osc8_consumer_keeps_semicolons_ids_and_the_2047_byte_boundary(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        let mut config = Config::default();
        config.cursor_blink = false;
        cx.set_global(config);
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, _window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));

    let mut bytes = Vec::new();
    // Semicolon-bearing destination.
    bytes.extend_from_slice(b"\x1b[5;1H\x1b]8;;https://example.com/a;b\x1b\\LINK\x1b]8;;\x1b\\");
    // Repeated ids: the last non-empty one is the stored id.
    bytes.extend_from_slice(b"\x1b[6;1H\x1b]8;id=first:id=last;https://two\x1b\\ID\x1b]8;;\x1b\\");
    // Exactly 2047 combined bytes: empty params + ';' + 2046-byte URI.
    let at_limit = "u".repeat(2046);
    bytes.extend_from_slice(b"\x1b[7;1H");
    bytes.extend_from_slice(b"\x1b]8;;");
    bytes.extend_from_slice(at_limit.as_bytes());
    bytes.extend_from_slice(b"\x1b\\A\x1b]8;;\x1b\\");
    // 2048 combined bytes is ignored: it neither opens nor closes, so the
    // next cell still carries the link that was open before it.
    let over_limit = "u".repeat(2047);
    bytes.extend_from_slice(b"\x1b[8;1H\x1b]8;;https://kept\x1b\\E");
    bytes.extend_from_slice(b"\x1b]8;;");
    bytes.extend_from_slice(over_limit.as_bytes());
    bytes.extend_from_slice(b"\x1b\\B\x1b]8;;\x1b\\C");
    // An empty URI with an id is ignored by this target version: the link
    // stays open for the next cell.
    bytes.extend_from_slice(
        b"\x1b[9;1H\x1b]8;;https://kept2\x1b\\F\x1b]8;id=x;\x1b\\G\x1b]8;;\x1b\\H",
    );
    // An empty URI without an id closes: the next cell is unlinked.
    bytes.extend_from_slice(b"\x1b[10;1H\x1b]8;;https://closed\x1b\\L\x1b]8;;\x1b\\M");

    graphics_send_fixture(&view, cx, 1, &bytes);

    let semicolon_link = hyperlink_at(&view, cx, 4, 0).expect("the link opens");
    assert_eq!(
        semicolon_link.1, "https://example.com/a;b",
        "the semicolon is part of the destination"
    );
    assert!(
        !semicolon_link.0.is_empty(),
        "a link sent without an id still gets a synthesized one"
    );
    assert_eq!(
        hyperlink_at(&view, cx, 5, 0),
        Some(("last".to_string(), "https://two".to_string())),
        "the last non-empty repeated id wins"
    );
    let boundary = hyperlink_at(&view, cx, 6, 0).expect("2047 bytes opens a link");
    assert_eq!(boundary.1.len(), 2046);
    assert_eq!(boundary.1, at_limit);
    assert_eq!(
        hyperlink_at(&view, cx, 7, 0).map(|link| link.1),
        Some("https://kept".to_string()),
        "the cell before the oversized report keeps its link"
    );
    assert_eq!(
        hyperlink_at(&view, cx, 7, 1).map(|link| link.1),
        Some("https://kept".to_string()),
        "an oversized report neither opens nor closes"
    );
    assert_eq!(
        hyperlink_at(&view, cx, 7, 2),
        None,
        "the explicit close after the oversized report still works"
    );
    assert_eq!(
        hyperlink_at(&view, cx, 8, 1).map(|link| link.1),
        Some("https://kept2".to_string()),
        "an empty URI with an id is ignored, leaving the link open"
    );
    assert_eq!(hyperlink_at(&view, cx, 8, 2), None);
    assert_eq!(
        hyperlink_at(&view, cx, 9, 0).map(|link| link.1),
        Some("https://closed".to_string())
    );
    assert_eq!(
        hyperlink_at(&view, cx, 9, 1),
        None,
        "an empty URI without an id closes the link"
    );
}
