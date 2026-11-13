// Real PTY output -> live daemon -> ordered pane stream -> production sidebar.
#[gpui_kit::test]
fn progress_consumer_real_pty_tab_retains_stopped_sender_after_ring_trim(cx: &mut TestAppContext) {
    use crate::daemon::protocol::{PaneProgress, ProgressKind};
    use crate::ui::{
        app::LyrelApp,
        pane::{Pane, PaneSlot},
        tab::Tab,
    };
    use gpui_kit::test::TestWindowExt as _;
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.cursor_blink = false;
        cx.set_global(cfg);
    });
    let attach = |vcx: &mut gpui_kit::VisualTestContext| {
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
            LyrelApp::watch_terminal(&view, window, cx);
            app.tabs.clear();
            app.tabs
                .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
            app.tabs.select(0);
            cx.notify();
            view
        })
    };
    let view = attach(&mut vcx);
    for (serial, bytes, kind, percent, key) in [
        (
            27101,
            b"\x1b]9;4;1;42\x07".as_slice(),
            ProgressKind::Running,
            Some(42),
            L10nKey::TabProgressRunningPercent,
        ),
        (
            27102,
            b"\x1b]9;4;3\x07".as_slice(),
            ProgressKind::Indeterminate,
            None,
            L10nKey::TabProgressIndeterminate,
        ),
        (
            27103,
            b"\x1b]9;4;4;61\x07".as_slice(),
            ProgressKind::Paused,
            Some(61),
            L10nKey::TabProgressPausedPercent,
        ),
        (
            27104,
            b"\x1b]9;4;2\x07".as_slice(),
            ProgressKind::Error,
            None,
            L10nKey::TabProgressError,
        ),
    ] {
        graphics_send_fixture(&view, cx, serial, bytes);
        graphics_recovery_wait(cx, "progress reached production consumer", |cx| {
            view.read_with(cx, |view, _| {
                view.progress() == Some(PaneProgress::new(kind, percent))
            })
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let status = window.find("tab-progress");
            assert!(status.visible());
            let expected = percent.map_or_else(
                || t(key).to_string(),
                |percent| t_fmt(key, &[("percent", &percent.to_string())]),
            );
            assert_eq!(status.label(), Some(expected.as_str()));
        });
        assert!(!view.read_with(cx, |view, _| view.agent_result_unread()));
    }
    // A finite file is written by the real PTY producer once, then the producer
    // blocks on stdin. No progress byte is repeated during or after eviction.
    let file = daemon.dir.path().join("progress-ring-trim.txt");
    std::fs::write(&file, b"replay filler\r\n".repeat(700_000)).unwrap();
    let request = format!(
        "FILE_SEND 27105 0 {} 8192\n",
        graphics_pty_fixture::hex(file.to_str().unwrap().as_bytes())
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(&view, cx, 27105, input_bytes);
    let mut observer = daemon
        .panes()
        .observe(
            pane,
            crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
        )
        .unwrap();
    observer
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut replay_bytes = 0;
    loop {
        match observer.recv().unwrap() {
            DaemonMsg::Snapshot(bytes) => {
                replay_bytes += bytes.len();
                assert!(
                    !bytes.windows(6).any(|part| part == b"]9;4;2"),
                    "original progress bytes must be evicted"
                );
            }
            DaemonMsg::Progress(state) => {
                assert_eq!(state, Some(PaneProgress::new(ProgressKind::Error, None)));
                break;
            }
            _ => {}
        }
    }
    assert!(
        replay_bytes > 7 * 1024 * 1024,
        "production ring was not forced full: {replay_bytes}"
    );
    observer.detach().unwrap();
    drop(view);
    let fresh = attach(&mut vcx);
    graphics_recovery_wait(
        cx,
        "fresh consumer restored error without sender resend",
        |cx| {
            fresh.read_with(cx, |view, _| {
                view.progress() == Some(PaneProgress::new(ProgressKind::Error, None))
            })
        },
    );
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("tab-progress").label(),
            Some(t(L10nKey::TabProgressError))
        );
    });
    assert!(fresh.read_with(cx, |view, _| view.progress()).is_some());
    assert!(!fresh.read_with(cx, |view, _| view.agent_result_unread()));
    graphics_send_fixture(&fresh, cx, 27106, b"\x1b]133;C\x07");
    graphics_recovery_wait(cx, "next command clears retained error", |cx| {
        fresh.read_with(cx, |view, _| view.progress().is_none())
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("tab-progress").is_none());
    });
}

#[gpui_kit::test]
fn progress_consumer_persisted_text_is_not_authority_and_hidden_duplicates_do_not_repaint(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::{PaneProgress, ProgressKind};
    use std::{cell::Cell, rc::Rc};
    let (window, mut daemon) = harness_on(cx, PtySource::Raw);
    let pane = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    cx.update(|cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(
            "progress-clipboard-sentinel".into(),
        ));
        declare_displayed(cx, [(pane.entity_id(), false)]);
    });
    DaemonMsg::Snapshot(
        b"\x1b]9;4;1;82\x07\x1b]52;c;Y2hhbmdlZA==\x07\x07persisted-progress-end".to_vec(),
    )
    .encode(&mut daemon)
    .unwrap();
    graphics_recovery_wait(cx, "persisted text arrived", |cx| {
        pane.read_with(cx, |view, _| {
            view.scrollback_text().contains("persisted-progress-end")
        })
    });
    pane.read_with(cx, |view, _| {
        assert_eq!(
            view.progress(),
            None,
            "old text must not resurrect progress in a replacement shell"
        );
        assert!(!view.bell_flash, "snapshot must not replay the bell");
    });
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("progress-clipboard-sentinel")
    );
    let semantic = Rc::new(Cell::new(0));
    let notifications = Rc::new(Cell::new(0));
    let event_count = semantic.clone();
    let notify_count = notifications.clone();
    let _semantic = cx.update(|cx| {
        cx.subscribe(&pane, move |_, _: &ProgressChanged, _| {
            event_count.set(event_count.get() + 1)
        })
    });
    let _notifications =
        cx.update(|cx| cx.observe(&pane, move |_, _| notify_count.set(notify_count.get() + 1)));
    let state = Some(PaneProgress::new(ProgressKind::Running, Some(42)));
    DaemonMsg::Progress(state).encode(&mut daemon).unwrap();
    graphics_recovery_wait(cx, "authoritative progress arrived", |cx| {
        pane.read_with(cx, |view, _| view.progress() == state)
    });
    cx.run_until_parked();
    assert_eq!(semantic.get(), 1);
    for _ in 0..100 {
        DaemonMsg::Progress(state).encode(&mut daemon).unwrap();
        DaemonMsg::Output(b"hidden ordinary output\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
    }
    DaemonMsg::Cwd(std::path::PathBuf::from("/progress-duplicate-barrier"))
        .encode(&mut daemon)
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if pane.read_with(cx, |view, _| {
            view.cwd() == Some(std::path::PathBuf::from("/progress-duplicate-barrier"))
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    cx.run_until_parked();
    assert_eq!(
        semantic.get(),
        1,
        "identical reports are not meaningful changes"
    );
    assert_eq!(
        notifications.get(),
        0,
        "hidden output must not invalidate the terminal surface"
    );
}
