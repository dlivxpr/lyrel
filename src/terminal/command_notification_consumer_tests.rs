// Real PTY output -> isolated production daemon -> ordered pane stream ->
// production workbench. Notifications are observed at the OS-delivery boundary
// (the sanitized title/body production hands to the backend), not by reading
// the view's completion fields.

const COMMAND_NOTIFICATION_PROMPT: &[u8] = b"\x1b]133;A\x07\x1b]133;B\x07";

fn command_notification_send(
    view: &gpui_kit::Entity<TerminalView>,
    cx: &mut gpui_kit::TestAppContext,
    serial: &mut usize,
    bytes: &[u8],
) {
    *serial += 1;
    graphics_send_fixture(view, cx, *serial, bytes);
}

fn command_notification_wait_running(
    view: &gpui_kit::Entity<TerminalView>,
    cx: &mut gpui_kit::TestAppContext,
) {
    graphics_recovery_wait(cx, "the command start reached the production pane", |cx| {
        view.read_with(cx, |view, _| !view.terminal.at_prompt())
    });
}

fn command_notification_wait_result(
    view: &gpui_kit::Entity<TerminalView>,
    cx: &mut gpui_kit::TestAppContext,
    exit: Option<i32>,
) {
    graphics_recovery_wait(cx, "the command result reached the production pane", |cx| {
        view.read_with(cx, |view, _| {
            view.terminal.at_prompt() && view.terminal.last_exit_code() == exit
        })
    });
}

/// The foreground poll runs every 300 ms in production. Advancing the executor
/// clock invokes that same lifecycle task instead of reaching into the view.
fn command_notification_pump(cx: &mut gpui_kit::TestAppContext) {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(310));
    cx.run_until_parked();
}

/// One ordinary command through a real PTY: the prompt's ownership marks, the
/// command-start mark, then the completion mark carrying `exit` (or none when
/// `None` stands for a shell that omitted the status).
fn command_notification_cycle(
    view: &gpui_kit::Entity<TerminalView>,
    cx: &mut gpui_kit::TestAppContext,
    serial: &mut usize,
    exit: Option<i32>,
) {
    command_notification_send(view, cx, serial, COMMAND_NOTIFICATION_PROMPT);
    graphics_recovery_wait(cx, "the prompt reached the production pane", |cx| {
        view.read_with(cx, |view, _| view.terminal.at_prompt())
    });

    command_notification_send(view, cx, serial, b"\x1b]133;C;fixture;aid=lyrel\x07");
    command_notification_wait_running(view, cx);
    command_notification_pump(cx);

    let completion = match exit {
        Some(code) => format!("\x1b]133;D;{code};aid=lyrel\x07").into_bytes(),
        None => b"\x1b]133;D;aid=lyrel\x07".to_vec(),
    };
    command_notification_send(view, cx, serial, &completion);
    command_notification_wait_result(view, cx, exit);
    command_notification_pump(cx);
}

fn command_notification_take() -> Vec<crate::terminal::desktop_notification::DeliveredNotification>
{
    crate::terminal::desktop_notification::take_test_notifications()
}

fn command_notification_attach(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> gpui_kit::Entity<TerminalView> {
    let stream = daemon.stream(pane);
    app.update_in(vcx, |app, window, cx| {
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
        crate::ui::app::LyrelApp::watch_terminal(&view, window, cx);
        app.tabs.clear();
        app.tabs
            .push(crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
                crate::ui::pane::PaneSlot::Ready(view.clone()),
            )));
        app.tabs.select(0);
        let focus = view.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
        view
    })
}

/// Put the fixture at a prompt *before* the GUI attaches. Otherwise the first
/// foreground poll sees an active shell that has not reported a prompt yet and
/// mistakes it for a command in flight, which would notify on the very first
/// prompt and confuse the command results under test.
fn command_notification_prime_prompt(daemon: &GraphicsDaemonProcess, pane: u64) {
    let mut stream = daemon.stream(pane);
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    // The fixture echoes a complete SEND request, not raw bytes: its line
    // reader needs the newline and its graphics barrier keeps the real daemon
    // reply path exercised before the GUI joins.
    let request = format!(
        "SEND 1 0 {} 0\n",
        graphics_pty_fixture::hex(COMMAND_NOTIFICATION_PROMPT)
    );
    ClientMsg::Input(request.into_bytes())
        .encode(&mut stream)
        .unwrap();
    let marker = b"KITTY_DONE_1;";
    let mut prompt_seen = false;
    let mut fixture_done = false;
    let mut output = Vec::new();
    while !(prompt_seen && fixture_done) {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Prompt {
                active: true,
                at_prompt: true,
                ..
            } => prompt_seen = true,
            DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                output.extend(bytes);
                fixture_done |= output.windows(marker.len()).any(|window| window == marker);
            }
            DaemonMsg::Error(error) => panic!("the prompt fixture was refused: {error}"),
            _ => {}
        }
    }
    ClientMsg::Detach.encode(&mut stream).unwrap();
}

/// Wait for the attach replay's prompt without moving the executor clock, so
/// the first foreground poll starts from a settled prompt.
fn command_notification_wait_replayed_prompt(
    view: &gpui_kit::Entity<TerminalView>,
    cx: &mut gpui_kit::TestAppContext,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !view.read_with(cx, |view, _| view.terminal.at_prompt()) {
        assert!(
            std::time::Instant::now() < deadline,
            "the replayed prompt never reached the production pane"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn real_pty_command_results_notify_with_existing_policy(cx: &mut gpui_kit::TestAppContext) {
    let _capture = crate::terminal::desktop_notification::capture_test_notifications();
    let _ = command_notification_take();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    command_notification_prime_prompt(&daemon, pane);
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.cursor_blink = false;
        cfg.notify_on_command_finish = crate::core::config::NotifyMode::Always;
        cfg.notify_threshold_secs = 0;
        cx.set_global(cfg);
    });
    let view = command_notification_attach(&daemon, pane, &app, &mut vcx);
    command_notification_wait_replayed_prompt(&view, cx);
    let pane_entity = view.entity_id();
    crate::ui::i18n::set_locale("en");
    // The primer used serial 1, so the first command cycle starts above it:
    // a reused marker would already be in the replayed scrollback.
    let mut serial = 1;
    let _ = command_notification_take();

    command_notification_cycle(&view, cx, &mut serial, Some(0));
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "the successful command notified once: {notes:?}"
    );
    assert_eq!(
        notes[0].pane,
        Some(pane_entity),
        "the toast still reveals the pane"
    );
    assert_eq!(
        notes[0].title, "lyrel",
        "the local pane keeps its app title"
    );
    assert!(
        notes[0].body.contains("succeeded after "),
        "zero is success, not the old neutral wording: {:?}",
        notes[0].body
    );
    assert!(
        !notes[0].body.contains("exit code"),
        "success does not invent a failure code: {:?}",
        notes[0].body
    );

    command_notification_cycle(&view, cx, &mut serial, Some(7));
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "the failed command notified once: {notes:?}"
    );
    assert!(
        notes[0].body.contains("failed after ") && notes[0].body.contains("exit code 7"),
        "a known nonzero status names its code: {:?}",
        notes[0].body
    );

    command_notification_cycle(&view, cx, &mut serial, None);
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "the omitted status notified once: {notes:?}"
    );
    assert!(
        notes[0].body.contains("exit status unknown"),
        "an absent code is explicitly unknown, never success: {:?}",
        notes[0].body
    );
    assert!(
        !notes[0].body.contains("succeeded"),
        "missing data must not be read as success: {:?}",
        notes[0].body
    );

    // The existing duration policy still suppresses short commands even when
    // the shell reports a definitive result.
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.notify_threshold_secs = 3600;
        cx.set_global(cfg);
    });
    command_notification_cycle(&view, cx, &mut serial, Some(0));
    assert!(
        command_notification_take().is_empty(),
        "a command under the configured threshold stays quiet"
    );

    // `never` remains a hard gate, independent of the delivery path.
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.notify_on_command_finish = crate::core::config::NotifyMode::Never;
        cfg.notify_threshold_secs = 0;
        cx.set_global(cfg);
    });
    command_notification_cycle(&view, cx, &mut serial, Some(0));
    assert!(
        command_notification_take().is_empty(),
        "notify_on_command_finish=never suppresses ordinary completion"
    );

    // The default `unfocused` policy still waits for the window to lose
    // activity; this harness window is active, so completion stays quiet.
    vcx.update(|window, cx| {
        window.activate_window();
        let mut cfg = cx.global::<Config>().clone();
        cfg.notify_on_command_finish = crate::core::config::NotifyMode::Unfocused;
        cx.set_global(cfg);
    });
    let active = vcx.update(|window, _| window.is_window_active());
    assert!(
        active,
        "the unfocused-mode case needs an active window to stay quiet"
    );
    command_notification_cycle(&view, cx, &mut serial, Some(0));
    assert!(
        command_notification_take().is_empty(),
        "unfocused mode stays quiet while the active window is watching this pane"
    );

    // The ordinary path stands on its own and follows the existing locale.
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.notify_on_command_finish = crate::core::config::NotifyMode::Always;
        cx.set_global(cfg);
    });
    crate::ui::i18n::set_locale("zh-CN");
    command_notification_cycle(&view, cx, &mut serial, Some(0));
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "the localized command notified once: {notes:?}"
    );
    assert!(
        notes[0].body.contains("执行成功"),
        "the same path uses the Simplified Chinese wording: {:?}",
        notes[0].body
    );
    crate::ui::i18n::set_locale("en");
}
