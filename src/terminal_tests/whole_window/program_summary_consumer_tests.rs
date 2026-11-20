// Real OSC 7501 output over isolated daemon PTYs -> one production tab ->
// the sidebar row and the attributed hover card's own data.
//
// The tests observe the presentation the workbench actually builds: the row's
// registered status element and `Tab::program_summary`, which is what the row
// and its card render from. No private sorter, field assignment or source text
// is consulted.

fn program_summary_attach(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> gpui_kit::Entity<TerminalView> {
    let stream = daemon.stream(pane);
    app.update_in(vcx, |_app, window, cx| {
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
        view
    })
}

fn program_summary_show_tab(
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
    views: &[gpui_kit::Entity<TerminalView>],
) {
    use crate::ui::pane::{Pane, PaneSlot};
    let first = views.first().expect("a tab needs one pane").clone();
    app.update_in(vcx, |app, window, cx| {
        app.tabs.clear();
        let mut pane = Pane::leaf(PaneSlot::Ready(first.clone()));
        for view in &views[1..] {
            pane = Pane::split_node(
                gpui_kit::Axis::Horizontal,
                0.5,
                pane,
                Pane::leaf(PaneSlot::Ready(view.clone())),
            );
        }
        app.tabs.push(crate::ui::tab::Tab::new(pane));
        app.tabs.select(0);
        let focus = first.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    });
    vcx.run_until_parked();
}

fn program_summary_wait_records(
    cx: &mut TestAppContext,
    view: &gpui_kit::Entity<TerminalView>,
    records: usize,
    reason: &str,
) {
    graphics_recovery_wait(cx, reason, |cx| {
        view.read_with(cx, |view, _| {
            view.with_program_status(|status| status.records().count() == records)
        })
    });
}

fn program_summary_wait_representative(
    cx: &mut TestAppContext,
    view: &gpui_kit::Entity<TerminalView>,
    id: &str,
    reason: &str,
) {
    graphics_recovery_wait(cx, reason, |cx| {
        view.read_with(cx, |view, _| {
            view.with_program_status(|status| {
                status
                    .representative()
                    .is_some_and(|record| record.id() == id)
            })
        })
    });
}

/// The row's own registered status element, as a reader sees it.
fn program_summary_row(vcx: &mut gpui_kit::VisualTestContext) -> Option<String> {
    use gpui_kit::test::TestWindowExt as _;
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window
            .try_find(("sidebar-program-status", 0usize))
            .filter(|status| status.visible())
            .and_then(|status| status.label().map(str::to_owned))
    })
}

fn program_summary_expected(key: lyrel_ui_base::i18n::L10nKey) -> String {
    use lyrel_ui_base::i18n::{L10nKey, t};
    format!("{} · {}", t(L10nKey::ProgramSource), t(key))
}

fn program_summary_of(
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> std::rc::Rc<crate::ui::program_summary::ProgramSummary> {
    app.read_with(vcx, |app, cx| app.tabs[0].program_summary(cx))
        .expect("the tab presents a program summary")
}

#[gpui_kit::test]
fn program_summary_consumer_multi_pane_priority_attribution_parking_and_reattach(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    use lyrel_core::core::program_status::ProgramState;
    use lyrel_ui_base::i18n::L10nKey;
    lyrel_ui_base::i18n::set_locale("en");
    let daemon = GraphicsDaemonProcess::start();
    let left = daemon.spawn();
    let right = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let left_view = program_summary_attach(&daemon, left, &app, &mut vcx);
    let right_view = program_summary_attach(&daemon, right, &app, &mut vcx);
    program_summary_show_tab(&app, &mut vcx, &[left_view.clone(), right_view.clone()]);

    // Working outranks done even though the done record arrived later.
    graphics_send_fixture(
        &left_view,
        cx,
        31501,
        b"\x1b]7501;state=working:app=builder\x07",
    );
    graphics_send_fixture(
        &right_view,
        cx,
        31502,
        b"\x1b]7501;state=done:app=tests\x07",
    );
    program_summary_wait_records(cx, &left_view, 1, "left working record");
    program_summary_wait_records(cx, &right_view, 1, "right done record");
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramWorking).as_str())
    );
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(summary.representative().pane_id(), left);
    assert_eq!(
        summary.representative().record().state(),
        ProgramState::Working
    );
    assert!(
        summary.others().iter().any(|entry| {
            entry.pane_id() == right && entry.record().state() == ProgramState::Done
        })
    );

    // A blocked child inherits the right pane's app and outranks working.
    graphics_send_fixture(
        &right_view,
        cx,
        31503,
        b"\x1b]7501;state=blocked:kind=permission:id=build/check:msg=QXBwcm92ZQ==\x07",
    );
    program_summary_wait_records(cx, &right_view, 2, "right blocked child");
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramPermission).as_str())
    );
    let summary = program_summary_of(&app, &mut vcx);
    let representative = summary.representative();
    assert_eq!(representative.pane_id(), right);
    assert_eq!(representative.record().id(), "build/check");
    assert_eq!(representative.record().app(), Some("tests"));
    let label = representative.tooltip_label();
    assert!(
        label.starts_with(&format!("#{right} · id build/check · ")),
        "card attributes the originating pane and record: {label}"
    );
    assert!(label.contains("tests"), "card names the program: {label}");
    assert!(label.contains("Permission needed"), "{label}");
    let other = summary
        .others()
        .iter()
        .find(|entry| entry.pane_id() == left)
        .expect("the left working record stays listed under the blocked child");
    assert!(
        other.tooltip_label().contains(&format!("#{left}")),
        "the remaining record keeps its own pane attribution: {}",
        other.tooltip_label()
    );
    assert!(other.tooltip_label().contains("builder"));

    // OSC 9;4 keeps its own line and its own authority: a program record and a
    // progress report render side by side instead of one displacing the other.
    graphics_send_fixture(&left_view, cx, 31507, b"\x1b]9;4;1;42\x07");
    graphics_recovery_wait(cx, "progress report reaches the left pane", |cx| {
        left_view.read_with(cx, |view, _| {
            view.progress()
                == Some(crate::daemon::protocol::PaneProgress::new(
                    crate::daemon::protocol::ProgressKind::Running,
                    Some(42),
                ))
        })
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let progress = window.find(("sidebar-progress-status", 0usize));
        assert!(progress.visible());
        assert_eq!(
            progress.label(),
            Some(
                lyrel_ui_base::i18n::t_fmt(
                    L10nKey::TabProgressRunningPercent,
                    &[("percent", "42")]
                )
                .as_str()
            )
        );
        let program = window.find(("sidebar-program-status", 0usize));
        assert!(program.visible());
        assert_eq!(
            program.label(),
            Some(program_summary_expected(L10nKey::ProgramPermission).as_str())
        );
    });

    // Error outranks working but not blocked.
    graphics_send_fixture(&left_view, cx, 31504, b"\x1b]7501;state=error:id=build\x07");
    program_summary_wait_records(cx, &left_view, 2, "left error child");
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramPermission).as_str())
    );
    // Clearing the blocked subtree hands the row to the error.
    graphics_send_fixture(
        &right_view,
        cx,
        31505,
        b"\x1b]7501;state=clear:id=build\x07",
    );
    program_summary_wait_records(cx, &right_view, 1, "right blocked subtree cleared");
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramError).as_str())
    );

    // A parked leaf is still a logical pane of this tab: its program state is
    // included while the pane itself is a hidden card.
    graphics_send_fixture(
        &right_view,
        cx,
        31506,
        b"\x1b]7501;state=blocked:kind=auth:id=login\x07",
    );
    program_summary_wait_records(cx, &right_view, 2, "right auth record");
    app.update_in(&mut vcx, |app, window, cx| {
        app.park_pane(right_view.entity_id(), window, cx)
    });
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert!(
            app.tabs[0].is_parked_slot(&crate::ui::pane::PaneSlot::Ready(right_view.clone()), cx)
        );
    });
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramAuth).as_str())
    );

    let before = right_view.read_with(cx, |view, _| {
        view.with_program_status(|status| {
            (
                status.session_id().map(str::to_owned),
                status.semantic_version(),
                status.result_version(),
            )
        })
    });

    // Removing the pane from the tree drops it from the answer immediately.
    program_summary_show_tab(&app, &mut vcx, std::slice::from_ref(&left_view));
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramError).as_str())
    );
    drop(right_view);

    // A fresh attachment restores current state from the daemon baseline,
    // keeping the session identity and version high-water it already had.
    let fresh = program_summary_attach(&daemon, right, &app, &mut vcx);
    program_summary_wait_records(cx, &fresh, 2, "reattached program baseline");
    let after = fresh.read_with(cx, |view, _| {
        view.with_program_status(|status| {
            (
                status.session_id().map(str::to_owned),
                status.semantic_version(),
                status.result_version(),
            )
        })
    });
    assert_eq!(after.0, before.0, "session identity survives reattachment");
    assert!(
        after.1 >= before.1 && after.2 >= before.2,
        "version counters never regress across reattachment: {before:?} -> {after:?}"
    );
    program_summary_show_tab(&app, &mut vcx, &[left_view, fresh]);
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramAuth).as_str())
    );
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(summary.representative().record().id(), "login");
    assert_eq!(summary.representative().record().app(), Some("tests"));
}

#[gpui_kit::test]
fn program_summary_consumer_identical_refresh_moves_a_tie_without_a_new_result(
    cx: &mut TestAppContext,
) {
    use lyrel_ui_base::i18n::L10nKey;
    lyrel_ui_base::i18n::set_locale("en");
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let view = program_summary_attach(&daemon, pane, &app, &mut vcx);
    program_summary_show_tab(&app, &mut vcx, std::slice::from_ref(&view));

    graphics_send_fixture(
        &view,
        cx,
        31511,
        b"\x1b]7501;state=done:app=checks:id=first:msg=Rmlyc3Q=\x07",
    );
    program_summary_wait_records(cx, &view, 1, "first done record");
    graphics_send_fixture(
        &view,
        cx,
        31512,
        b"\x1b]7501;state=done:app=checks:id=second:msg=U2Vjb25k\x07",
    );
    program_summary_wait_records(cx, &view, 2, "second done record");
    let counters = view.read_with(cx, |view, _| {
        view.with_program_status(|status| (status.semantic_version(), status.result_version()))
    });
    // Both records report done; the later one holds the tie.
    program_summary_wait_representative(cx, &view, "second", "newer done record wins the tie");
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(summary.representative().record().id(), "second");
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramDone).as_str())
    );

    // An identical report is not a new result. It only refreshes daemon
    // recency, so the representative flips while both counters stay put —
    // proving the tie is not reconstructed from content.
    graphics_send_fixture(
        &view,
        cx,
        31513,
        b"\x1b]7501;state=done:app=checks:id=first:msg=Rmlyc3Q=\x07",
    );
    program_summary_wait_representative(cx, &view, "first", "recency-only tie change");
    let after = view.read_with(cx, |view, _| {
        view.with_program_status(|status| (status.semantic_version(), status.result_version()))
    });
    assert_eq!(after, counters, "identical refresh adds no semantic/result");
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(summary.representative().record().id(), "first");
    assert!(
        summary
            .others()
            .iter()
            .any(|entry| entry.record().id() == "second"),
        "the displaced tie is still listed"
    );
    assert!(
        summary
            .representative()
            .tooltip_label()
            .contains("id first")
    );

    // And back again, still without a new result.
    graphics_send_fixture(
        &view,
        cx,
        31514,
        b"\x1b]7501;state=done:app=checks:id=second:msg=U2Vjb25k\x07",
    );
    program_summary_wait_representative(cx, &view, "second", "second recency-only tie change");
    let after = view.read_with(cx, |view, _| {
        view.with_program_status(|status| (status.semantic_version(), status.result_version()))
    });
    assert_eq!(
        after, counters,
        "either direction of the tie is recency-only"
    );

    // No current records means no row line at all; idle is the one state
    // below done and still reports in words.
    graphics_send_fixture(&view, cx, 31515, b"\x1b]7501;state=clear\x07");
    program_summary_wait_records(cx, &view, 0, "clear removes current records");
    vcx.run_until_parked();
    assert_eq!(program_summary_row(&mut vcx), None);
    graphics_send_fixture(&view, cx, 31516, b"\x1b]7501;state=idle:app=checks\x07");
    program_summary_wait_records(cx, &view, 1, "idle record");
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramIdle).as_str())
    );

    // The card is bounded: the representative plus six others are listed and
    // the rest is only counted, never laid out.
    let mut burst = Vec::new();
    for index in 0..9 {
        burst.extend_from_slice(format!("\x1b]7501;state=working:id=r{index}\x07").as_bytes());
    }
    graphics_send_fixture(&view, cx, 31517, &burst);
    program_summary_wait_records(cx, &view, 10, "nine more current records");
    vcx.run_until_parked();
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(
        summary.representative().record().state(),
        lyrel_core::core::program_status::ProgramState::Working
    );
    assert_eq!(summary.others().len(), 6);
    assert_eq!(summary.hidden(), 3);
}

#[gpui_kit::test]
fn program_summary_consumer_same_pane_representative_refresh_beats_another_pane(
    cx: &mut TestAppContext,
) {
    use lyrel_ui_base::i18n::L10nKey;
    lyrel_ui_base::i18n::set_locale("en");
    let daemon = GraphicsDaemonProcess::start();
    let left = daemon.spawn();
    let right = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let left_view = program_summary_attach(&daemon, left, &app, &mut vcx);
    let right_view = program_summary_attach(&daemon, right, &app, &mut vcx);
    program_summary_show_tab(&app, &mut vcx, &[left_view.clone(), right_view.clone()]);

    // Two equivalent done records: the later daemon report holds the tie.
    graphics_send_fixture(&left_view, cx, 31531, b"\x1b]7501;state=done:app=alpha\x07");
    program_summary_wait_records(cx, &left_view, 1, "left done record");
    graphics_send_fixture(&right_view, cx, 31532, b"\x1b]7501;state=done:app=beta\x07");
    program_summary_wait_records(cx, &right_view, 1, "right done record");
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(summary.representative().pane_id(), right);

    // An identical report refreshes only the left record's daemon recency. The
    // pane's own representative id stays the empty root id and no result is
    // produced, yet the tab's cross-pane tie must move to the left pane.
    let left_before = left_view.read_with(cx, |view, _| {
        view.with_program_status(|status| {
            (
                status.representative().map(|record| record.id().to_owned()),
                status.semantic_version(),
                status.result_version(),
            )
        })
    });
    graphics_send_fixture(&left_view, cx, 31533, b"\x1b]7501;state=done:app=alpha\x07");
    graphics_recovery_wait(cx, "identical left refresh wins the cross-pane tie", |cx| {
        app.read_with(cx, |app, cx| {
            app.tabs[0].program_summary(cx).is_some_and(|summary| {
                summary.representative().pane_id() == left
                    && summary.representative().record().id().is_empty()
            })
        })
    });
    let left_after = left_view.read_with(cx, |view, _| {
        view.with_program_status(|status| {
            (
                status.representative().map(|record| record.id().to_owned()),
                status.semantic_version(),
                status.result_version(),
            )
        })
    });
    assert_eq!(
        left_after, left_before,
        "the same pane keeps its representative id and produces no new result"
    );
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramDone).as_str())
    );
    let summary = program_summary_of(&app, &mut vcx);
    assert_eq!(summary.representative().pane_id(), left);
    assert!(
        summary
            .others()
            .iter()
            .any(|entry| entry.pane_id() == right)
    );
}

#[gpui_kit::test]
fn program_summary_consumer_keeps_agent_authority_beside_program_state(cx: &mut TestAppContext) {
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
    use lyrel_core::core::program_status::ProgramState;
    use lyrel_ui_base::i18n::L10nKey;
    lyrel_ui_base::i18n::set_locale("en");
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let program = program_summary_attach(&daemon, pane, &app, &mut vcx);
    let (agent, mut agent_daemon) = app.update_in(&mut vcx, |_app, window, cx| {
        let (view, stream) = lyrel_terminal::view::quiet_test_pane(9_315, window, cx);
        crate::ui::app::LyrelApp::watch_terminal(&view, window, cx);
        (view, stream)
    });
    program_summary_show_tab(&app, &mut vcx, &[program.clone(), agent.clone()]);

    DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
        .encode(&mut agent_daemon)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        status: AgentStatus::Done,
        rich: true,
        turns: 1,
        ..Default::default()
    })))
    .encode(&mut agent_daemon)
    .unwrap();
    graphics_recovery_wait(cx, "agent session reaches the production view", |cx| {
        agent.read_with(cx, |view, _| {
            view.agent_session().map(|session| session.status) == Some(AgentStatus::Done)
        })
    });
    agent.update(&mut vcx, |view, cx| {
        view.mark_agent_result_unread(false, cx)
    });

    graphics_send_fixture(
        &program,
        cx,
        31521,
        b"\x1b]7501;state=blocked:kind=question:msg=QW5zd2Vy\x07",
    );
    program_summary_wait_records(cx, &program, 1, "blocked program record");
    vcx.run_until_parked();
    assert_eq!(
        program_summary_row(&mut vcx).as_deref(),
        Some(program_summary_expected(L10nKey::ProgramQuestion).as_str())
    );
    // The agent's own authority is untouched by the program line in the same
    // tab: both read from their own leaves, and the row draws both.
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs[0].agent_status(cx), Some(AgentStatus::Done));
        assert_eq!(app.tabs[0].agent_unread_count(cx), 1);
        let summary = app.tabs[0]
            .program_summary(cx)
            .expect("blocked program summary");
        assert_eq!(
            summary.representative().record().state(),
            ProgramState::Blocked
        );
    });
}
