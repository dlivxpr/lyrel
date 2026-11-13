use super::progress::summarize_progress;
use super::*;
use crate::daemon::protocol::{DaemonMsg, PaneProgress, ProgressKind};
use crate::daemon::transport::Stream;
use crate::terminal::view::TerminalView;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Axis, Entity, SharedString, TestAppContext};

fn wait_until(cx: &mut TestAppContext, mut ready: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready(cx) {
        assert!(
            std::time::Instant::now() < deadline,
            "tab progress condition was not reached"
        );
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn progress(kind: ProgressKind, percent: Option<u8>) -> PaneProgress {
    PaneProgress::new(kind, percent)
}

fn send(stream: &mut Stream, state: Option<PaneProgress>) {
    DaemonMsg::Progress(state).encode(stream).unwrap();
}

fn wait_state(view: &Entity<TerminalView>, cx: &mut TestAppContext, state: Option<PaneProgress>) {
    let view = view.clone();
    wait_until(cx, move |cx| {
        view.read_with(cx, |view, _| view.progress() == state)
    });
}

fn tab_label(
    app: &Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
    index: usize,
) -> Option<(ProgressKind, SharedString)> {
    app.update_in(vcx, |app, _, cx| app.tabs[index].progress_label(cx))
}

/// Move two other tabs' panes into `index`, leaving one three-pane split.
fn split_tab(app: &mut crate::ui::app::LyrelApp, index: usize) -> [PaneSlot; 3] {
    let a = app.tabs[index].pane.first_leaf().expect("tab has a pane");
    let b = app.tabs.remove(index + 1).pane.first_leaf().expect("pane");
    let c = app.tabs.remove(index + 1).pane.first_leaf().expect("pane");
    assert!(
        app.tabs[index]
            .pane
            .split_leaf(a.entity_id(), Axis::Horizontal, false, b.clone(),)
    );
    assert!(
        app.tabs[index]
            .pane
            .split_leaf(b.entity_id(), Axis::Vertical, false, c.clone(),)
    );
    app.tabs.select(index);
    [a, b, c]
}

#[test]
fn a_single_task_keeps_its_state_and_percentage() {
    for (kind, percent) in [
        (ProgressKind::Running, Some(42)),
        (ProgressKind::Running, None),
        (ProgressKind::Indeterminate, Some(3)),
        (ProgressKind::Indeterminate, None),
        (ProgressKind::Paused, Some(61)),
        (ProgressKind::Paused, None),
        (ProgressKind::Error, Some(7)),
        (ProgressKind::Error, None),
    ] {
        assert_eq!(
            summarize_progress([progress(kind, percent)]),
            Some((kind, percent))
        );
    }
}

#[test]
fn no_reports_mean_no_tab_progress() {
    assert_eq!(summarize_progress(std::iter::empty()), None);
}

#[test]
fn several_tasks_never_become_an_average() {
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Running, Some(42)),
            progress(ProgressKind::Running, Some(70)),
        ]),
        Some((ProgressKind::Running, None))
    );
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Indeterminate, None),
            progress(ProgressKind::Paused, Some(70)),
        ]),
        Some((ProgressKind::Indeterminate, None))
    );
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Running, Some(42)),
            progress(ProgressKind::Indeterminate, None),
            progress(ProgressKind::Paused, Some(70)),
        ]),
        Some((ProgressKind::Running, None))
    );
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Paused, Some(10)),
            progress(ProgressKind::Paused, Some(20)),
        ]),
        Some((ProgressKind::Paused, None))
    );
}

#[test]
fn errors_outrank_every_other_task() {
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Error, Some(61)),
            progress(ProgressKind::Running, Some(42)),
        ]),
        Some((ProgressKind::Error, None))
    );
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Running, Some(42)),
            progress(ProgressKind::Error, None),
            progress(ProgressKind::Paused, Some(70)),
        ]),
        Some((ProgressKind::Error, None))
    );
    assert_eq!(
        summarize_progress([
            progress(ProgressKind::Error, Some(1)),
            progress(ProgressKind::Error, Some(2)),
        ]),
        Some((ProgressKind::Error, None))
    );
}

#[gpui_kit::test]
fn split_tab_progress_reads_every_leaf(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
    let right = app.update_in(&mut vcx, |app, _window, _cx| {
        let left = app.tabs[0].pane.first_leaf().expect("first tab has a pane");
        let right = app
            .tabs
            .remove(1)
            .pane
            .first_leaf()
            .expect("second tab has a pane");
        assert!(app.tabs[0].pane.split_leaf(
            left.entity_id(),
            Axis::Horizontal,
            false,
            right.clone(),
        ));
        app.tabs.select(0);
        right.terminal().cloned().expect("the pane is ready")
    });
    // The second tab's pane is the second stream the harness handed back.
    send(
        &mut streams[1],
        Some(progress(ProgressKind::Running, Some(42))),
    );
    wait_state(&right, cx, Some(progress(ProgressKind::Running, Some(42))));
    let (kind, label) =
        tab_label(&app, &mut vcx, 0).expect("a split tab still reports its pane's progress");
    assert_eq!(kind, ProgressKind::Running);
    let expected = t_fmt(
        L10nKey::TabProgressRunningPercent,
        &[("percent", &42.to_string())],
    );
    assert_eq!(label.as_ref(), expected.as_str());
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 0usize)).label(),
            Some(expected.as_str())
        );
    });
}

#[gpui_kit::test]
fn split_tab_progress_prefers_errors_and_never_averages(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 3);
    let [a, b, c] = app.update_in(&mut vcx, |app, _window, _cx| split_tab(app, 0));
    let a = a.terminal().cloned().expect("ready");
    let b = b.terminal().cloned().expect("ready");
    let c = c.terminal().cloned().expect("ready");

    send(
        &mut streams[0],
        Some(progress(ProgressKind::Running, Some(42))),
    );
    send(
        &mut streams[1],
        Some(progress(ProgressKind::Indeterminate, None)),
    );
    send(
        &mut streams[2],
        Some(progress(ProgressKind::Error, Some(61))),
    );
    wait_state(&a, cx, Some(progress(ProgressKind::Running, Some(42))));
    wait_state(&b, cx, Some(progress(ProgressKind::Indeterminate, None)));
    wait_state(&c, cx, Some(progress(ProgressKind::Error, Some(61))));
    let (kind, label) = tab_label(&app, &mut vcx, 0).expect("three tasks");
    assert_eq!(kind, ProgressKind::Error);
    assert_eq!(label.as_ref(), t(L10nKey::TabProgressError));
    assert!(!label.as_ref().contains('%'), "no invented average");

    // One task left: its own number is honest again.
    send(&mut streams[2], None);
    wait_state(&c, cx, None);
    send(&mut streams[1], None);
    wait_state(&b, cx, None);
    let (kind, label) = tab_label(&app, &mut vcx, 0).expect("one task remains");
    assert_eq!(kind, ProgressKind::Running);
    assert_eq!(
        label.as_ref(),
        t_fmt(
            L10nKey::TabProgressRunningPercent,
            &[("percent", &42.to_string())]
        )
    );

    // Error still leads while a second task is paused.
    send(
        &mut streams[0],
        Some(progress(ProgressKind::Error, Some(61))),
    );
    send(
        &mut streams[1],
        Some(progress(ProgressKind::Paused, Some(70))),
    );
    wait_state(&a, cx, Some(progress(ProgressKind::Error, Some(61))));
    wait_state(&b, cx, Some(progress(ProgressKind::Paused, Some(70))));
    let (kind, label) = tab_label(&app, &mut vcx, 0).expect("error plus paused");
    assert_eq!(kind, ProgressKind::Error);
    assert_eq!(label.as_ref(), t(L10nKey::TabProgressError));

    send(&mut streams[0], None);
    wait_state(&a, cx, None);
    let (kind, label) = tab_label(&app, &mut vcx, 0).expect("one paused task");
    assert_eq!(kind, ProgressKind::Paused);
    assert_eq!(
        label.as_ref(),
        t_fmt(
            L10nKey::TabProgressPausedPercent,
            &[("percent", &70.to_string())]
        )
    );

    send(
        &mut streams[2],
        Some(progress(ProgressKind::Indeterminate, None)),
    );
    send(&mut streams[1], None);
    wait_state(&c, cx, Some(progress(ProgressKind::Indeterminate, None)));
    wait_state(&b, cx, None);
    let (kind, label) = tab_label(&app, &mut vcx, 0).expect("one indeterminate task");
    assert_eq!(kind, ProgressKind::Indeterminate);
    assert_eq!(label.as_ref(), t(L10nKey::TabProgressIndeterminate));

    send(&mut streams[2], None);
    wait_state(&c, cx, None);
    assert_eq!(tab_label(&app, &mut vcx, 0), None);
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
fn tab_progress_follows_park_focus_close_and_move(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 3);
    let [a, b, c] = app.update_in(&mut vcx, |app, _window, _cx| split_tab(app, 0));
    let a = a.terminal().cloned().expect("ready");
    let b = b.terminal().cloned().expect("ready");
    let _c = c.terminal().cloned().expect("ready");

    app.update_in(&mut vcx, |_app, window, cx| {
        let handle = a.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    send(
        &mut streams[1],
        Some(progress(ProgressKind::Running, Some(42))),
    );
    wait_state(&b, cx, Some(progress(ProgressKind::Running, Some(42))));
    assert_eq!(
        tab_label(&app, &mut vcx, 0)
            .expect("the unfocused leaf reports")
            .0,
        ProgressKind::Running
    );

    let parked_expected = t_fmt(
        L10nKey::TabProgressRunningPercent,
        &[("percent", &42.to_string())],
    );
    app.update_in(&mut vcx, |app, _window, cx| {
        assert!(app.tabs[0].park(b.entity_id(), app.workspace, cx).is_some());
        assert!(app.tabs[0].is_parked_slot(&PaneSlot::Ready(b.clone()), cx));
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 0usize)).label(),
            Some(parked_expected.as_str())
        );
    });
    assert_eq!(
        tab_label(&app, &mut vcx, 0)
            .expect("a parked leaf is still part of the tab")
            .0,
        ProgressKind::Running
    );

    app.update_in(&mut vcx, |app, _window, cx| {
        assert!(
            app.tabs[0]
                .restore_parked(b.entity_id(), app.workspace, cx)
                .is_some()
        );
        assert!(!app.tabs[0].is_parked_slot(&PaneSlot::Ready(b.clone()), cx));
    });
    app.update_in(&mut vcx, |_app, window, cx| {
        let handle = b.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    assert_eq!(
        tab_label(&app, &mut vcx, 0)
            .expect("focus changes do not change membership")
            .0,
        ProgressKind::Running
    );

    app.update_in(&mut vcx, |app, _window, _cx| {
        app.tabs[0].pane.close_leaf(b.entity_id());
    });
    assert_eq!(
        tab_label(&app, &mut vcx, 0),
        None,
        "closing the reporting pane retires its progress"
    );

    send(
        &mut streams[0],
        Some(progress(ProgressKind::Running, Some(15))),
    );
    wait_state(&a, cx, Some(progress(ProgressKind::Running, Some(15))));
    let _other_stream = app.update_in(&mut vcx, |app, window, cx| {
        let (other, stream) = crate::terminal::view::quiet_test_pane(90, window, cx);
        crate::ui::app::LyrelApp::watch_terminal(&other, window, cx);
        app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(other))));
        app.tabs.select(0);
        let moved = Pane::Leaf(PaneSlot::Ready(a.clone()));
        app.tabs[0].pane.close_leaf(a.entity_id());
        let dst = app.tabs[1].pane.first_leaf().expect("destination pane");
        assert!(
            app.tabs[1]
                .pane
                .graft_beside(moved, &dst, Dir::Right)
                .is_ok()
        );
        cx.notify();
        stream
    });
    assert_eq!(
        tab_label(&app, &mut vcx, 0),
        None,
        "the source tab no longer reports the moved pane"
    );
    let (kind, label) = tab_label(&app, &mut vcx, 1).expect("the moved pane reports");
    assert_eq!(kind, ProgressKind::Running);
    assert_eq!(
        label.as_ref(),
        t_fmt(
            L10nKey::TabProgressRunningPercent,
            &[("percent", &15.to_string())]
        )
    );
}

#[gpui_kit::test]
fn hidden_tab_progress_updates_summary_without_view_notifications(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
    let hidden = app.read_with(cx, |app, _| {
        app.tabs[1]
            .pane
            .first_leaf()
            .expect("hidden tab has a pane")
            .terminal()
            .cloned()
            .expect("ready")
    });
    let notifications = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let counter = notifications.clone();
    let _subscription = cx.update(|cx| {
        cx.observe(&hidden, move |_, _| {
            counter.set(counter.get() + 1);
        })
    });
    notifications.set(0);
    send(
        &mut streams[1],
        Some(progress(ProgressKind::Running, Some(42))),
    );
    wait_state(&hidden, cx, Some(progress(ProgressKind::Running, Some(42))));
    assert_eq!(
        notifications.get(),
        0,
        "progress is a semantic update, not a terminal repaint"
    );
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 1usize)).label(),
            Some(
                t_fmt(
                    L10nKey::TabProgressRunningPercent,
                    &[("percent", &42.to_string())]
                )
                .as_str()
            )
        );
    });

    send(
        &mut streams[1],
        Some(progress(ProgressKind::Running, Some(43))),
    );
    wait_state(&hidden, cx, Some(progress(ProgressKind::Running, Some(43))));
    assert_eq!(notifications.get(), 0);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("sidebar-progress-status", 1usize)).label(),
            Some(
                t_fmt(
                    L10nKey::TabProgressRunningPercent,
                    &[("percent", &43.to_string())]
                )
                .as_str()
            )
        );
    });
}

#[gpui_kit::test]
fn tab_progress_preserves_agent_waiting_and_unread(cx: &mut TestAppContext) {
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    let view = app.read_with(cx, |app, _| {
        app.tabs[0]
            .pane
            .first_leaf()
            .expect("tab has a pane")
            .terminal()
            .cloned()
            .expect("ready")
    });
    let session = |status, turns| AgentSessionState {
        status,
        message: None,
        session_id: Some("sid-272".into()),
        launch_argv: Some(vec!["omp".into()]),
        rich: true,
        cwd: None,
        activity: 0,
        turns,
        pending_requests: Vec::new(),
        request_resume_status: Default::default(),
        pending_stop: false,
        readout: Default::default(),
        since_ms: None,
    };
    DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
        .encode(&mut streams[0])
        .unwrap();
    DaemonMsg::AgentStatus(Some(session(AgentStatus::Waiting, 0)))
        .encode(&mut streams[0])
        .unwrap();
    wait_until(cx, |cx| {
        view.read_with(cx, |view, _| {
            view.agent() == Some(CLIAgent::OhMyPi)
                && view.agent_session().map(|s| s.status) == Some(AgentStatus::Waiting)
        })
    });

    send(
        &mut streams[0],
        Some(progress(ProgressKind::Running, Some(42))),
    );
    wait_state(&view, cx, Some(progress(ProgressKind::Running, Some(42))));
    app.update_in(&mut vcx, |app, _window, cx| {
        assert_eq!(app.tabs[0].agent_status(cx), Some(AgentStatus::Waiting));
        assert_eq!(app.tabs[0].agent_unread_count(cx), 0);
        assert!(
            app.tabs[0].progress_label(cx).is_some(),
            "progress supplements waiting for input"
        );
    });

    DaemonMsg::AgentStatus(Some(session(AgentStatus::Done, 1)))
        .encode(&mut streams[0])
        .unwrap();
    wait_until(cx, |cx| {
        view.read_with(cx, |view, _| {
            view.agent_session().map(|s| s.status) == Some(AgentStatus::Done)
        })
    });
    view.update(cx, |view, cx| view.mark_agent_result_unread(false, cx));
    app.update_in(&mut vcx, |app, _window, cx| {
        assert_eq!(app.tabs[0].agent_status(cx), Some(AgentStatus::Done));
        assert_eq!(app.tabs[0].agent_unread_count(cx), 1);
        assert!(
            app.tabs[0].progress_label(cx).is_some(),
            "progress does not erase a completed, unread turn"
        );
    });

    send(&mut streams[0], None);
    wait_state(&view, cx, None);
    app.update_in(&mut vcx, |app, _window, cx| {
        assert_eq!(
            app.tabs[0].agent_unread_count(cx),
            1,
            "clearing progress does not clear agent unread"
        );
    });
}
