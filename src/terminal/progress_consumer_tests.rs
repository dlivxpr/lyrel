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
            let status = window.find(("sidebar-progress-status", 0usize));
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
            window.find(("sidebar-progress-status", 0usize)).label(),
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
        assert!(
            window
                .try_find(("sidebar-progress-status", 0usize))
                .is_none()
        );
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

#[gpui_kit::test]
fn progress_consumer_real_persisted_screen_after_daemon_death_never_seeds_replacement_shell(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::{RestoreFrom, ShellSpec, WinSize};
    use crate::ui::{
        app::LyrelApp,
        pane::{Pane, PaneSlot},
        tab::Tab,
    };
    use gpui_kit::test::TestWindowExt as _;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let old = GraphicsDaemonProcess::start();
    let old_pane = old.spawn();
    let (old_view, old_window) = graphics_rooted_consumer(cx, old_pane, old.stream(old_pane));
    graphics_send_fixture(
        &old_view,
        cx,
        27120,
        b"\x1b]9;4;1;42\x07\r\nPERSISTED_PROGRESS_TEXT\r\n",
    );
    let size = WinSize {
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    };
    let mut observer = old.panes().observe(old_pane, size).unwrap();
    observer
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut geometry = size;
    let mut segments = Vec::new();
    loop {
        match observer.recv().unwrap() {
            DaemonMsg::Size(size) => geometry = size,
            DaemonMsg::Snapshot(bytes) => segments.push(crate::daemon::scrollback::Segment {
                size: geometry,
                bytes,
            }),
            DaemonMsg::Progress(Some(_)) => break,
            _ => {}
        }
    }
    observer.detach().unwrap();
    assert!(
        segments.iter().any(|segment| segment
            .bytes
            .windows(10)
            .any(|bytes| bytes == b"]9;4;1;42\x07")),
        "the persisted fixture must contain the original producer's progress sequence"
    );
    let persisted = crate::daemon::scrollback::encode(&segments, None);
    drop(old); // Only this test's isolated daemon; the old PTY authority is gone.
    graphics_recovery_destroy(old_view, old_window, cx);
    let replacement = GraphicsDaemonProcess::start_with(|_, dir| {
        let scrollback = dir.join("scrollback");
        std::fs::create_dir_all(&scrollback).unwrap();
        std::fs::write(scrollback.join(format!("{old_pane}.bin")), &persisted).unwrap();
    });
    let mut stream =
        crate::daemon::transport::connect_endpoint_at(&replacement.endpoint("daemon")).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
    ClientMsg::Spawn {
        cwd: None,
        size,
        shell: Some(ShellSpec {
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
        }),
        owner: None,
        workspace: None,
        restore: Some(RestoreFrom {
            pane_id: old_pane,
            banner: Some("replacement shell".into()),
        }),
        allow_remote_clipboard_write: false,
        await_claim: false,
    }
    .encode(&mut stream)
    .unwrap();
    let DaemonMsg::Spawned { pane_id } = DaemonMsg::read(&mut stream).unwrap() else {
        panic!("replacement PTY did not spawn")
    };
    stream.set_read_timeout(None).unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let view = app.update_in(&mut vcx, |app, window, cx| {
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
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane_id, window, cx));
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
        app.tabs.select(0);
        cx.notify();
        view
    });
    // A unique live barrier follows the authoritative restore baseline. The
    // old screen also contains KITTY_READY, so that text alone proves nothing.
    graphics_send_fixture(&view, cx, 27122, b"\r\nREPLACEMENT_PRODUCER_READY\r\n");
    graphics_recovery_wait(
        cx,
        "actual disk restore and replacement PTY reached production tab",
        |cx| {
            view.read_with(cx, |view, _| {
                let text = view.scrollback_text();
                text.contains("PERSISTED_PROGRESS_TEXT")
                    && text.contains("REPLACEMENT_PRODUCER_READY")
            })
        },
    );
    assert_eq!(view.read_with(cx, |view, _| view.progress()), None);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("sidebar-progress-status", 0usize))
                .is_none()
        );
    });
    graphics_send_fixture(&view, cx, 27121, b"\x1b]9;4;4;61\x07");
    graphics_recovery_wait(cx, "new producer establishes only its own progress", |cx| {
        view.read_with(cx, |view, _| view.progress().is_some())
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 0usize)).label(),
            Some(t_fmt(L10nKey::TabProgressPausedPercent, &[("percent", "61")]).as_str())
        );
    });
}

/// The tab, not the focused pane, owns the summary: two real PTY producers
/// feed one split tab and the split reports the work of both.
#[gpui_kit::test]
fn progress_consumer_split_tab_reads_real_pty_producers(cx: &mut TestAppContext) {
    use crate::daemon::protocol::{PaneProgress, ProgressKind};
    use crate::ui::{
        app::LyrelApp,
        pane::{Pane, PaneSlot},
        tab::Tab,
    };
    use gpui_kit::Axis;
    use gpui_kit::test::TestWindowExt as _;

    let daemon = GraphicsDaemonProcess::start();
    let pane_a = daemon.spawn();
    let pane_b = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.cursor_blink = false;
        cx.set_global(cfg);
    });
    let views = app.update_in(&mut vcx, |app, window, cx| {
        let terminal = RemoteTerminal::from_stream_with(
            daemon.stream(pane_a),
            TermSize::new(80, 24),
            Vec::new(),
            if cfg!(windows) {
                PtySource::LocalConpty
            } else {
                PtySource::Raw
            },
        )
        .unwrap();
        let a = cx.new(|cx| TerminalView::with_terminal(terminal, pane_a, window, cx));
        LyrelApp::watch_terminal(&a, window, cx);
        let terminal = RemoteTerminal::from_stream_with(
            daemon.stream(pane_b),
            TermSize::new(80, 24),
            Vec::new(),
            if cfg!(windows) {
                PtySource::LocalConpty
            } else {
                PtySource::Raw
            },
        )
        .unwrap();
        let b = cx.new(|cx| TerminalView::with_terminal(terminal, pane_b, window, cx));
        LyrelApp::watch_terminal(&b, window, cx);
        let tree = Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Ready(a.clone())),
            Pane::Leaf(PaneSlot::Ready(b.clone())),
        );
        app.tabs.clear();
        app.tabs.push(Tab::new(tree));
        app.tabs.select(0);
        cx.notify();
        [a, b]
    });
    graphics_send_fixture(&views[0], cx, 27201, b"\x1b]9;4;1;42\x07");
    graphics_recovery_wait(cx, "split pane progress reached production consumer", |cx| {
        views[0].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Running, Some(42)))
        })
    });
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("a split tab reports its live pane's progress");
    assert_eq!(kind, ProgressKind::Running);
    let expected = t_fmt(L10nKey::TabProgressRunningPercent, &[("percent", "42")]);
    assert_eq!(label.as_ref(), expected.as_str());
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(("sidebar-progress-status", 0usize))
                .label(),
            Some(expected.as_str())
        );
    });
}

/// Three real PTY producers feed one split tab: the summary reports error
/// precedence, never an average, and follows per-pane command boundaries.
#[gpui_kit::test]
fn progress_consumer_split_tab_aggregates_real_pty_producers(cx: &mut TestAppContext) {
    use crate::daemon::protocol::{PaneProgress, ProgressKind};
    use crate::ui::{
        app::LyrelApp,
        pane::{Pane, PaneSlot},
        tab::Tab,
    };
    use gpui_kit::Axis;
    use gpui_kit::test::TestWindowExt as _;

    let daemon = GraphicsDaemonProcess::start();
    let panes = [daemon.spawn(), daemon.spawn(), daemon.spawn()];
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.cursor_blink = false;
        cx.set_global(cfg);
    });
    let views = app.update_in(&mut vcx, |app, window, cx| {
        let mut views = Vec::new();
        for pane in panes {
            let terminal = RemoteTerminal::from_stream_with(
                daemon.stream(pane),
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
            views.push(view);
        }
        let left = Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Ready(views[0].clone())),
            Pane::Leaf(PaneSlot::Ready(views[1].clone())),
        );
        let tree = Pane::split_node(
            Axis::Vertical,
            0.5,
            left,
            Pane::Leaf(PaneSlot::Ready(views[2].clone())),
        );
        app.tabs.clear();
        app.tabs.push(Tab::new(tree));
        app.tabs.select(0);
        cx.notify();
        views
    });
    graphics_send_fixture(&views[0], cx, 27201, b"\x1b]9;4;1;42\x07");
    graphics_send_fixture(&views[1], cx, 27202, b"\x1b]9;4;3\x07");
    graphics_send_fixture(&views[2], cx, 27203, b"\x1b]9;4;2;61\x07");
    graphics_recovery_wait(cx, "three real PTY producers reached the split tab", |cx| {
        views[0].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Running, Some(42)))
        }) && views[1].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Indeterminate, None))
        }) && views[2].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Error, Some(61)))
        })
    });
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("the split tab summarizes its leaves");
    assert_eq!(kind, ProgressKind::Error);
    assert_eq!(label.as_ref(), t(L10nKey::TabProgressError));
    assert!(!label.as_ref().contains('%'), "no fabricated average");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 0usize)).label(),
            Some(t(L10nKey::TabProgressError))
        );
    });

    graphics_send_fixture(&views[1], cx, 27204, b"\x1b]9;4;0\x07");
    graphics_send_fixture(&views[2], cx, 27205, b"\x1b]9;4;0\x07");
    graphics_recovery_wait(cx, "explicit clear retired two tasks", |cx| {
        views[1].read_with(cx, |view, _| view.progress().is_none())
            && views[2].read_with(cx, |view, _| view.progress().is_none())
    });
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("one task remains");
    assert_eq!(kind, ProgressKind::Running);
    let expected = t_fmt(L10nKey::TabProgressRunningPercent, &[("percent", "42")]);
    assert_eq!(label.as_ref(), expected.as_str());

    // A reliable command end retires running work in its own pane.
    graphics_send_fixture(&views[0], cx, 27206, b"\x1b]133;D;0\x07");
    graphics_recovery_wait(cx, "command end cleared running progress", |cx| {
        views[0].read_with(cx, |view, _| view.progress().is_none())
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx)),
        None
    );

    // An error survives the command end, and the next command retires it.
    graphics_send_fixture(&views[0], cx, 27207, b"\x1b]9;4;2;61\x07");
    graphics_recovery_wait(cx, "error reached the pane", |cx| {
        views[0].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Error, Some(61)))
        })
    });
    graphics_send_fixture(&views[0], cx, 27208, b"\x1b]133;D;0\x07");
    graphics_send_fixture(&views[0], cx, 27209, b"\x1b[0m");
    assert_eq!(
        views[0].read_with(cx, |view, _| view.progress()),
        Some(PaneProgress::new(ProgressKind::Error, Some(61))),
        "a command end must retain the error"
    );
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("the retained error is the single task");
    assert_eq!(kind, ProgressKind::Error);
    let expected = t_fmt(L10nKey::TabProgressErrorPercent, &[("percent", "61")]);
    assert_eq!(label.as_ref(), expected.as_str());
    graphics_send_fixture(&views[0], cx, 27210, b"\x1b]133;C\x07");
    graphics_recovery_wait(cx, "next command cleared the retained error", |cx| {
        views[0].read_with(cx, |view, _| view.progress().is_none())
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx)),
        None
    );
}

/// Parked, unfocused and moved leaves remain tab members; closed ones leave.
#[gpui_kit::test]
fn progress_consumer_split_tab_membership_follows_park_close_and_move(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::{PaneProgress, ProgressKind};
    use crate::ui::{
        app::LyrelApp,
        pane::{Dir, Pane, PaneSlot},
        tab::Tab,
    };
    use gpui_kit::Axis;

    let daemon = GraphicsDaemonProcess::start();
    let panes = [daemon.spawn(), daemon.spawn(), daemon.spawn()];
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.cursor_blink = false;
        cx.set_global(cfg);
    });
    let views = app.update_in(&mut vcx, |app, window, cx| {
        let mut views = Vec::new();
        for pane in panes {
            let terminal = RemoteTerminal::from_stream_with(
                daemon.stream(pane),
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
            views.push(view);
        }
        let left = Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Ready(views[0].clone())),
            Pane::Leaf(PaneSlot::Ready(views[1].clone())),
        );
        let tree = Pane::split_node(
            Axis::Vertical,
            0.5,
            left,
            Pane::Leaf(PaneSlot::Ready(views[2].clone())),
        );
        app.tabs.clear();
        app.tabs.push(Tab::new(tree));
        app.tabs.select(0);
        cx.notify();
        views
    });
    graphics_send_fixture(&views[0], cx, 27220, b"\x1b]9;4;1;42\x07");
    graphics_send_fixture(&views[1], cx, 27221, b"\x1b]9;4;2;61\x07");
    graphics_recovery_wait(cx, "two real PTY producers reached the tab", |cx| {
        views[0].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Running, Some(42)))
        }) && views[1].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Error, Some(61)))
        })
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
            .expect("two tasks")
            .0,
        ProgressKind::Error
    );

    app.update_in(&mut vcx, |app, _window, cx| {
        assert!(
            app.tabs[0]
                .park(views[1].entity_id(), app.workspace, cx)
                .is_some()
        );
        assert!(app.tabs[0].is_parked_slot(&PaneSlot::Ready(views[1].clone()), cx));
    });
    graphics_send_fixture(&views[0], cx, 27222, b"\x1b]9;4;0\x07");
    graphics_recovery_wait(cx, "the visible task cleared", |cx| {
        views[0].read_with(cx, |view, _| view.progress().is_none())
    });
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("the parked leaf still reports");
    assert_eq!(kind, ProgressKind::Error);
    let expected = t_fmt(L10nKey::TabProgressErrorPercent, &[("percent", "61")]);
    assert_eq!(label.as_ref(), expected.as_str());

    app.update_in(&mut vcx, |app, _window, cx| {
        assert!(
            app.tabs[0]
                .restore_parked(views[1].entity_id(), app.workspace, cx)
                .is_some()
        );
        assert!(!app.tabs[0].is_parked_slot(&PaneSlot::Ready(views[1].clone()), cx));
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
            .expect("unparking does not drop the task")
            .0,
        ProgressKind::Error
    );
    app.update_in(&mut vcx, |_app, window, cx| {
        let handle = views[2].read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
            .expect("focus changes do not change membership")
            .0,
        ProgressKind::Error
    );

    app.update_in(&mut vcx, |app, _window, _cx| {
        app.tabs[0].pane.close_leaf(views[1].entity_id());
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx)),
        None,
        "closing the error pane retires its progress"
    );

    graphics_send_fixture(&views[2], cx, 27223, b"\x1b]9;4;1;15\x07");
    graphics_recovery_wait(cx, "the moved pane has its own task", |cx| {
        views[2].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Running, Some(15)))
        })
    });
    let _other_stream = app.update_in(&mut vcx, |app, window, cx| {
        let (other, stream) = crate::terminal::view::quiet_test_pane(92, window, cx);
        LyrelApp::watch_terminal(&other, window, cx);
        app.tabs
            .push(Tab::new(Pane::leaf(PaneSlot::Ready(other))));
        app.tabs.select(0);
        let moved = Pane::Leaf(PaneSlot::Ready(views[2].clone()));
        app.tabs[0].pane.close_leaf(views[2].entity_id());
        let dst = app.tabs[1].pane.first_leaf().expect("destination pane");
        assert!(app.tabs[1].pane.graft_beside(moved, &dst, Dir::Right).is_ok());
        cx.notify();
        stream
    });
    assert_eq!(
        app.update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx)),
        None,
        "the source tab no longer reports the moved pane"
    );
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[1].progress_label(cx))
        .expect("the destination tab reports the moved pane");
    assert_eq!(kind, ProgressKind::Running);
    let expected = t_fmt(L10nKey::TabProgressRunningPercent, &[("percent", "15")]);
    assert_eq!(label.as_ref(), expected.as_str());
}

/// A sleeping tab has no live leaves; waking it against the same daemon
/// restores each pane's authoritative state into the reconstructed tab.
#[gpui_kit::test]
fn progress_consumer_split_tab_restores_after_sleep_reconstruction(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::{PaneProgress, ProgressKind};
    use crate::ui::{
        app::LyrelApp,
        pane::{Pane, PaneSlot},
        tab::Tab,
    };
    use gpui_kit::Axis;
    use gpui_kit::test::TestWindowExt as _;

    let daemon = GraphicsDaemonProcess::start();
    let panes = [daemon.spawn(), daemon.spawn()];
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.cursor_blink = false;
        cx.set_global(cfg);
    });
    let views = app.update_in(&mut vcx, |app, window, cx| {
        let mut views = Vec::new();
        for pane in panes {
            let terminal = RemoteTerminal::from_stream_with(
                daemon.stream(pane),
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
            views.push(view);
        }
        let tree = Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Ready(views[0].clone())),
            Pane::Leaf(PaneSlot::Ready(views[1].clone())),
        );
        app.tabs.clear();
        app.tabs.push(Tab::new(tree));
        app.tabs.select(0);
        cx.notify();
        views
    });
    graphics_send_fixture(&views[0], cx, 27230, b"\x1b]9;4;1;42\x07");
    graphics_send_fixture(&views[1], cx, 27231, b"\x1b]9;4;2;61\x07");
    graphics_recovery_wait(cx, "both live panes report", |cx| {
        views[0].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Running, Some(42)))
        }) && views[1].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Error, Some(61)))
        })
    });
    let presentation = app.update_in(&mut vcx, |app, _, _cx| app.tabs[0].presentation().clone());
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(crate::ui::session_lifecycle::SessionLifecycle::sleep(
            &mut app.tabs,
            0,
            window,
            cx
        ));
        assert!(app.tabs[0].is_asleep());
        assert!(
            app.tabs[0].progress_label(cx).is_none(),
            "a sleeping tab has no live progress"
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("sidebar-progress-status", 0usize)).is_none());
    });
    drop(views);

    let views = app.update_in(&mut vcx, |app, window, cx| {
        let mut views = Vec::new();
        for pane in panes {
            let terminal = RemoteTerminal::from_stream_with(
                daemon.stream(pane),
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
            views.push(view);
        }
        let tree = Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Ready(views[0].clone())),
            Pane::Leaf(PaneSlot::Ready(views[1].clone())),
        );
        app.tabs[0].finish_wake(tree, presentation);
        cx.notify();
        views
    });
    graphics_recovery_wait(cx, "the reconstructed split restored daemon progress", |cx| {
        views[0].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Running, Some(42)))
        }) && views[1].read_with(cx, |view, _| {
            view.progress() == Some(PaneProgress::new(ProgressKind::Error, Some(61)))
        })
    });
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("the reconstructed tab summarizes its leaves");
    assert_eq!(kind, ProgressKind::Error);
    assert_eq!(label.as_ref(), t(L10nKey::TabProgressError));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 0usize)).label(),
            Some(t(L10nKey::TabProgressError))
        );
    });
    graphics_send_fixture(&views[1], cx, 27232, b"\x1b]9;4;0\x07");
    graphics_recovery_wait(cx, "the restored error can be cleared", |cx| {
        views[1].read_with(cx, |view, _| view.progress().is_none())
    });
    let expected = t_fmt(L10nKey::TabProgressRunningPercent, &[("percent", "42")]);
    let (kind, label) = app
        .update_in(&mut vcx, |app, _, cx| app.tabs[0].progress_label(cx))
        .expect("the remaining task is exact");
    assert_eq!(kind, ProgressKind::Running);
    assert_eq!(label.as_ref(), expected.as_str());
}
