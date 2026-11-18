// Per-window observation from real public daemon streams, not injected frames.
fn program_observer_stream(daemon: &GraphicsDaemonProcess, pane: u64) -> Stream {
    let mut stream =
        crate::daemon::transport::connect_endpoint_at(&daemon.endpoint("daemon")).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    crate::daemon::protocol::negotiate_pane_rendering(&mut stream, true).unwrap();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: true,
    }
    .encode(&mut stream)
    .unwrap();
    ClientMsg::Observe {
        pane_id: pane,
        size: crate::daemon::protocol::WinSize {
            cols: 80,
            rows: 24,
            cell_w: 10,
            cell_h: 20,
        },
    }
    .encode(&mut stream)
    .unwrap();
    stream.set_read_timeout(None).unwrap();
    stream
}

fn program_assert_unread(vcx: &mut gpui_kit::VisualTestContext, expected: usize) {
    use gpui_kit::test::TestWindowExt as _;
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        if expected == 0 {
            assert!(
                window
                    .try_find(("sidebar-program-unread", 0usize))
                    .is_none()
            );
        } else {
            let label = format!("New results · {expected}");
            let reminder = window.find(("sidebar-program-unread", 0usize));
            assert!(reminder.visible());
            assert_eq!(reminder.label(), Some(label.as_str()));
        }
    });
}

#[gpui_kit::test]
fn program_attention_two_real_windows_ack_independently_and_reconnect_detects_new_result(
    cx: &mut TestAppContext,
) {
    use lyrel_core::core::program_status::ProgramState;
    let _capture = crate::terminal::desktop_notification::capture_test_notifications();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (first, mut av) = crate::ui::app::test_window::harness(cx);
    let (second, mut bv) = crate::ui::app::test_window::harness(cx);
    av.update(|_, cx| {
        cx.global_mut::<Config>().notify_on_command_finish =
            crate::core::config::NotifyMode::Always;
        cx.global_mut::<Config>().notify_threshold_secs = 3600;
    });
    crate::ui::i18n::set_locale("en");
    let a = program_consumer_attach(&daemon, pane, &first, &mut av);
    let b = program_consumer_attach_stream(
        program_observer_stream(&daemon, pane),
        pane,
        &second,
        &mut bv,
    );
    graphics_recovery_wait(cx, "both windows receive their cold baseline", |cx| {
        [&a, &b].iter().all(|view| {
            view.read_with(cx, |view, _| {
                view.with_program_status(|status| status.session_id().is_some())
            })
        })
    });
    av.deactivate_window();
    bv.deactivate_window();
    program_assert_unread(&mut av, 0);
    program_assert_unread(&mut bv, 0);
    assert!(
        command_notification_take().is_empty(),
        "a cold baseline is silent"
    );

    graphics_send_fixture(&a, cx, 751601, b"\x1b]7501;state=blocked:id=task:kind=permission:app=builder:msg=TmVlZHMgcGVybWlzc2lvbg==\x07");
    graphics_recovery_wait(cx, "both clients see the blocked result", |cx| {
        [&a, &b].iter().all(|view| {
            view.read_with(cx, |view, _| {
                view.with_program_status(|status| {
                    status
                        .representative()
                        .is_some_and(|r| r.state() == ProgramState::Blocked)
                })
            })
        })
    });
    program_assert_unread(&mut av, 1);
    program_assert_unread(&mut bv, 1);
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "two windows share one program-source OS notice: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .all(|note| note.body.contains("Permission needed"))
    );

    // The viewing window clears its own reminder, never the other window's
    // cursor or the daemon's blocked record. No approval is implied.
    av.update(|window, _| window.activate_window());
    program_assert_unread(&mut av, 0);
    program_assert_unread(&mut bv, 1);
    a.read_with(cx, |view, _| {
        view.with_program_status(|status| {
            assert_eq!(
                status.representative().unwrap().state(),
                ProgramState::Blocked
            );
        })
    });
    graphics_send_fixture(
        &a,
        cx,
        751602,
        b"\x1b]7501;state=blocked:id=task:kind=permission:app=builder:msg=UmVmcmVzaGVkIHRleHQ=\x07",
    );
    av.run_until_parked();
    bv.run_until_parked();
    program_assert_unread(&mut av, 0);
    assert!(
        command_notification_take().is_empty(),
        "routine text cannot re-notify or mark an acknowledged result unread"
    );

    graphics_send_fixture(
        &a,
        cx,
        751603,
        b"\x1b]7501;state=blocked:id=task:kind=question:app=builder\x07",
    );
    av.run_until_parked();
    bv.run_until_parked();
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "a blocked-kind transition notifies once across windows: {notes:?}"
    );
    assert!(notes.iter().all(|note| note.body.contains("Answer needed")));
    graphics_send_fixture(
        &a,
        cx,
        751604,
        b"\x1b]7501;state=done:id=task:app=builder:msg=RG9uZQ==\x07",
    );
    program_assert_unread(&mut av, 0);
    program_assert_unread(&mut bv, 1);
    assert!(
        command_notification_take().is_empty(),
        "done is visual, not another completion notice"
    );

    av.deactivate_window();
    bv.update(|window, _| window.activate_window());
    program_assert_unread(&mut bv, 0);
    let old_version = b.read_with(cx, |view, _| {
        view.with_program_status(|status| status.result_version())
    });
    // Drop only this observer; the shell and the first window remain alive.
    second.update_in(&mut bv, |app, _, cx| {
        app.tabs.clear();
        cx.notify();
    });
    drop(b);
    bv.deactivate_window();
    graphics_send_fixture(&a, cx, 751605, b"\x1b]7501;state=working:id=task:app=builder\x07\x1b]7501;state=done:id=task:app=builder:msg=RG9uZQ==\x07");
    graphics_recovery_wait(
        cx,
        "the disconnected cycle creates a new result identity",
        |cx| {
            a.read_with(cx, |view, _| {
                view.with_program_status(|status| status.result_version() > old_version)
            })
        },
    );
    let reattached = program_consumer_attach_stream(
        program_observer_stream(&daemon, pane),
        pane,
        &second,
        &mut bv,
    );
    bv.deactivate_window();
    graphics_recovery_wait(
        cx,
        "reconnected current result restores the newer version",
        |cx| {
            reattached.read_with(cx, |view, _| {
                view.with_program_status(|status| status.result_version() > old_version)
            })
        },
    );
    program_assert_unread(&mut bv, 1);
    program_assert_unread(&mut av, 1);
    assert!(
        command_notification_take().is_empty(),
        "reattachment never replays an OS notice"
    );
    bv.update(|window, _| window.activate_window());
    program_assert_unread(&mut bv, 0);
    program_assert_unread(&mut av, 1);
}

#[gpui_kit::test]
fn program_attention_cold_and_unchanged_reconnect_stay_quiet_but_recreated_results_are_new(
    cx: &mut TestAppContext,
) {
    let _capture = crate::terminal::desktop_notification::capture_test_notifications();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (first, mut av) = crate::ui::app::test_window::harness(cx);
    crate::ui::i18n::set_locale("en");
    av.update(|window, cx| {
        cx.global_mut::<Config>().notify_on_command_finish = crate::core::config::NotifyMode::Never;
        window.activate_window();
    });
    let a = program_consumer_attach(&daemon, pane, &first, &mut av);
    graphics_send_fixture(
        &a,
        cx,
        751620,
        b"\x1b]7501;state=done:id=cold:app=builder\x07",
    );
    program_summary_wait_records(cx, &a, 1, "the first window observes done");
    program_assert_unread(&mut av, 0);
    av.deactivate_window();

    let (second, mut bv) = crate::ui::app::test_window::harness(cx);
    bv.deactivate_window();
    let b = program_consumer_attach_stream(
        program_observer_stream(&daemon, pane),
        pane,
        &second,
        &mut bv,
    );
    program_summary_wait_records(cx, &b, 1, "the cold baseline contains the old done record");
    program_assert_unread(&mut bv, 0);
    second.update_in(&mut bv, |app, _, cx| {
        app.tabs.clear();
        cx.notify();
    });
    drop(b);
    let b = program_consumer_attach_stream(
        program_observer_stream(&daemon, pane),
        pane,
        &second,
        &mut bv,
    );
    program_summary_wait_records(cx, &b, 1, "unchanged reconnect restores the current record");
    program_assert_unread(&mut bv, 0);
    assert!(
        command_notification_take().is_empty(),
        "cold and unchanged baselines never replay OS notices"
    );

    graphics_send_fixture(
        &a,
        cx,
        751621,
        b"\x1b]7501;state=clear:id=cold\x07\x1b]7501;state=done:id=cold:app=builder\x07",
    );
    program_assert_unread(&mut av, 1);
    program_assert_unread(&mut bv, 1);
    av.update(|window, _| window.activate_window());
    program_assert_unread(&mut av, 0);
    program_assert_unread(&mut bv, 1);
    av.deactivate_window();

    graphics_send_fixture(&a, cx, 751622, b"\x1b]7501;state=clear\x07");
    program_summary_wait_records(cx, &a, 0, "clear retires the current result cursors");
    let mut reports = Vec::new();
    for ix in 0..256 {
        reports.extend_from_slice(format!("\x1b]7501;state=done:id=evict{ix}\x07").as_bytes());
    }
    graphics_send_fixture(&a, cx, 751623, &reports);
    program_summary_wait_records(cx, &a, 256, "the bounded public record table fills");
    program_summary_wait_records(cx, &b, 256, "the observer receives the same bounded table");
    program_assert_unread(&mut av, 256);
    program_assert_unread(&mut bv, 256);
    av.update(|window, _| window.activate_window());
    program_assert_unread(&mut av, 0);
    program_assert_unread(&mut bv, 256);
    av.deactivate_window();
    graphics_send_fixture(&a, cx, 751624, b"\x1b]7501;state=done:id=extra\x07");
    program_assert_unread(&mut av, 1);
    graphics_send_fixture(&a, cx, 751625, b"\x1b]7501;state=done:id=evict0\x07");
    program_assert_unread(&mut av, 2);
    program_assert_unread(&mut bv, 256);
    assert!(
        command_notification_take().is_empty(),
        "eviction/recreation of done stays visual only"
    );

    let old_session = a.read_with(cx, |view, _| {
        view.with_program_status(|status| status.session_id().unwrap().to_owned())
    });
    // A different isolated daemon starts its numeric pane ids at the same
    // value. Only the daemon session token can distinguish this new lifetime.
    let next_daemon = GraphicsDaemonProcess::start();
    let next_pane = next_daemon.spawn();
    assert_eq!(next_pane, pane);
    let next = program_consumer_attach(&next_daemon, next_pane, &first, &mut av);
    av.deactivate_window();
    graphics_recovery_wait(cx, "the reused pane id has a new session baseline", |cx| {
        next.read_with(cx, |view, _| {
            view.with_program_status(|status| {
                status
                    .session_id()
                    .is_some_and(|session| session != old_session)
            })
        })
    });
    program_assert_unread(&mut av, 0);
    graphics_send_fixture(&next, cx, 751626, b"\x1b]7501;state=done:id=evict0\x07");
    program_assert_unread(&mut av, 1);
    program_assert_unread(&mut bv, 256);
}

#[gpui_kit::test]
fn program_attention_budget_is_per_source_not_a_same_pane_mute(cx: &mut TestAppContext) {
    let _capture = crate::terminal::desktop_notification::capture_test_notifications();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    command_notification_prime_prompt(&daemon, pane);
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    crate::ui::i18n::set_locale("en");
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().notify_on_command_finish =
            crate::core::config::NotifyMode::Always;
        cx.global_mut::<Config>().notify_threshold_secs = 0;
    });
    let view = program_consumer_attach(&daemon, pane, &app, &mut vcx);
    command_notification_wait_replayed_prompt(&view, cx);
    vcx.deactivate_window();
    let mut burst = Vec::new();
    for ix in 0..7 {
        burst.extend_from_slice(
            format!("\x1b]7501;state=blocked:id=task{ix}:kind=permission\x07").as_bytes(),
        );
    }
    graphics_send_fixture(&view, cx, 751640, &burst);
    program_assert_unread(&mut vcx, 7);
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        crate::core::osc::NOTES_PER_WINDOW,
        "rotating untrusted record ids cannot bypass the program source budget: {notes:?}"
    );
    assert!(
        notes.iter().all(
            |note| note.title.starts_with("Program") && note.body.contains("Permission needed")
        )
    );

    // The exhausted OSC 7501 budget is not the shell completion budget.
    let mut serial = 751640;
    command_notification_cycle(&view, cx, &mut serial, Some(7));
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "an independent shell failure is not swallowed: {notes:?}"
    );
    assert!(notes[0].body.contains("exit code 7"));

    let other_pane = daemon.spawn();
    command_notification_prime_prompt(&daemon, other_pane);
    let other = program_summary_attach(&daemon, other_pane, &app, &mut vcx);
    program_summary_show_tab(&app, &mut vcx, &[view.clone(), other.clone()]);
    vcx.deactivate_window();
    graphics_send_fixture(
        &other,
        cx,
        751650,
        b"\x1b]7501;state=error:id=task0:app=independent\x07",
    );
    vcx.run_until_parked();
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        1,
        "another pane owns its own program budget: {notes:?}"
    );
    assert_eq!(notes[0].pane, Some(other.entity_id()));
    assert!(notes[0].body.contains("independent"));

    // Feed the production rich-hook ingress through the same real PTY. The
    // hook owns the agent result, not an independent OSC 7501 child task.
    graphics_send_fixture(&other, cx, 751651, b"\x1b]777;notify;lyrel://cli-agent;{\"v\":1,\"agent\":\"omp\",\"event\":\"session-start\",\"session_id\":\"program-attention-agent\"}\x07\x1b]777;notify;lyrel://cli-agent;{\"v\":1,\"agent\":\"omp\",\"event\":\"prompt-submit\",\"session_id\":\"program-attention-agent\"}\x07\x1b]133;C;fixture;aid=lyrel\x07");
    graphics_recovery_wait(cx, "the rich hook reports a working agent", |cx| {
        other.read_with(cx, |view, _| {
            view.agent_session().is_some_and(|session| {
                session.rich && session.status == crate::core::cli_agent::AgentStatus::Working
            })
        })
    });
    command_notification_pump(cx);
    assert!(
        command_notification_take().is_empty(),
        "starting a rich turn is not a result"
    );
    // The daemon samples the pane's foreground process on its own cadence.
    // That heuristic is evidence about the process table, not authority over
    // the turn: a rich session the agent's own hooks established must survive
    // a sample that cannot see the agent (a wrapper, a just-started process,
    // or this fixture pane). The sleep below only makes the next production
    // sample due; it is not a wait for success. The real PTY read after it
    // carries that sample, and every consumer pass below still asserts the
    // authoritative Working state.
    std::thread::sleep(std::time::Duration::from_millis(700));
    graphics_send_fixture(&other, cx, 751649, b"\x1b]2;probe-cadence\x07");
    for _ in 0..50 {
        graphics_consumer_progress(cx);
        let session = other.read_with(cx, |view, _| view.agent_session());
        assert!(
            session.as_ref().is_some_and(|session| {
                session.rich && session.status == crate::core::cli_agent::AgentStatus::Working
            }),
            "a foreground probe sample must not retire the rich turn: {session:?}"
        );
    }
    command_notification_pump(cx);
    assert!(
        command_notification_take().is_empty(),
        "a foreground probe sample is not a result"
    );
    graphics_send_fixture(&other, cx, 751652, b"\x1b]7501;state=error:id=unrelated:app=child-task\x07\x1b]777;notify;lyrel://cli-agent;{\"v\":1,\"agent\":\"omp\",\"event\":\"stop\",\"session_id\":\"program-attention-agent\",\"result\":{\"outcome\":\"succeeded\"}}\x07");
    graphics_recovery_wait(cx, "the rich hook retains its own completed result", |cx| {
        other.read_with(cx, |view, _| {
            view.agent_session().is_some_and(|session| {
                session.rich && session.status == crate::core::cli_agent::AgentStatus::Done
            })
        })
    });
    command_notification_pump(cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    let notes = command_notification_take();
    assert_eq!(
        notes.len(),
        2,
        "same-pane rich completion and independent program failure both notify: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|note| note.title.starts_with("Program") && note.body.contains("child-task"))
    );
    assert!(notes.iter().any(|note| {
        note.title
            .starts_with(crate::core::cli_agent::CLIAgent::OhMyPi.display_name())
    }));
    let mut serial = 751652;
    command_notification_send(&other, cx, &mut serial, b"\x1b]133;D;7;aid=lyrel\x07");
    command_notification_wait_result(&other, cx, Some(7));
    command_notification_pump(cx);
    assert!(
        command_notification_take().is_empty(),
        "existing rich-hook suppression still prevents duplicate shell completion"
    );
}
