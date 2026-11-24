// Real program output -> isolated daemon -> ordered stream -> production status bar.
fn program_consumer_attach(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    app: &Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> Entity<TerminalView> {
    program_consumer_attach_stream(daemon.stream(pane), pane, app, vcx)
}

fn program_consumer_attach_stream(
    stream: Stream,
    pane: u64,
    app: &Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> Entity<TerminalView> {
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
        .expect("attach the real program pane");
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

#[gpui_kit::test]
fn program_consumer_real_pty_current_state_and_reattach_use_daemon_records(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    use lyrel_core::core::program_status::ProgramState;
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    lyrel_ui_base::i18n::set_locale("en");
    let view = program_consumer_attach(&daemon, pane, &app, &mut vcx);
    graphics_send_fixture(
        &view,
        cx,
        750101,
        b"\x1b]7501;state=working:app=builder:progress=42:title=Q29tcGlsaW5n:msg=QnVpbGRpbmc=\x07",
    );
    graphics_recovery_wait(
        cx,
        "program representative reaches the current pane",
        |cx| {
            view.read_with(cx, |view, _| {
                view.with_program_status(|state| {
                    state
                        .representative()
                        .is_some_and(|record| record.state() == ProgramState::Working)
                })
            })
        },
    );
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let status = window.find("status-program");
        assert!(status.visible());
        // A named program omits the generic Program prefix (#402).
        assert_eq!(
            status.label(),
            Some("builder · Working · 42% · Compiling · Building")
        );
        assert!(window.find("status-program-dot").visible());
    });
    // A child inherits the live root app and outranks it. Invalid ids must not
    // replace the root; a second report proves the stream remains usable.
    graphics_send_fixture(&view, cx, 750102, b"\x1b]7501;state=error:id=bad//id\x07\x1b]7501;state=blocked:kind=question:id=build/check:msg=QW5zd2Vy\x07");
    graphics_recovery_wait(
        cx,
        "blocked child becomes the displayed representative",
        |cx| {
            view.read_with(cx, |view, _| {
                view.with_program_status(|state| {
                    state
                        .representative()
                        .is_some_and(|record| record.id() == "build/check")
                })
            })
        },
    );
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("status-program").label(),
            Some("builder · Answer needed · Answer")
        );
    });
    let before = view.read_with(cx, |view, _| {
        view.with_program_status(|state| {
            (
                state.session_id().map(str::to_owned),
                state.semantic_version(),
                state.result_version(),
            )
        })
    });
    // Attach a fresh production terminal consumer: state comes from a current
    // baseline, not replaying old OSC text or fabricating agent hook state.
    let fresh = program_consumer_attach(&daemon, pane, &app, &mut vcx);
    graphics_recovery_wait(cx, "new attachment restores authoritative state", |cx| {
        fresh.read_with(cx, |view, _| {
            view.with_program_status(|state| {
                state
                    .representative()
                    .is_some_and(|record| record.id() == "build/check")
            })
        })
    });
    fresh.read_with(cx, |view, _| {
        assert_eq!(
            view.with_program_status(|state| (
                state.session_id().map(str::to_owned),
                state.semantic_version(),
                state.result_version()
            )),
            before
        );
        assert!(
            view.agent_session().is_none(),
            "program reports never forge agent sessions"
        );
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("status-program").label(),
            Some("builder · Answer needed · Answer")
        );
    });
    graphics_send_fixture(&fresh, cx, 750103, b"\x1b]7501;state=clear:id=build\x07");
    graphics_recovery_wait(cx, "clearing the child restores the working root", |cx| {
        fresh.read_with(cx, |view, _| {
            view.with_program_status(|state| {
                state
                    .representative()
                    .is_some_and(|record| record.id().is_empty())
            })
        })
    });
    graphics_send_fixture(&fresh, cx, 750104, b"\x1bc");
    graphics_recovery_wait(cx, "RIS clears current program presentation", |cx| {
        fresh.read_with(cx, |view, _| {
            view.with_program_status(|state| state.representative().is_none())
        })
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("status-program").is_none());
    });
}
