//! Whole-workbench coverage for the app icon's system badge (#397): the
//! aggregate counts distinct eligible tabs across windows, the activity
//! matrix's badge column gates each row, and zero clears the platform label.

use crate::core::cli_agent::{
    AgentOutcome, AgentReadout, AgentResult, AgentSessionState, AgentStatus, CLIAgent,
};
use crate::daemon::protocol::DaemonMsg;
use crate::daemon::transport::Stream;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::program_attention::TerminalViewProgramSource as _;
use crate::ui::settings::ActivityChannel;
use crate::ui::system_badge::{SystemBadgeState, label_for, take_test_badges};
use crate::ui::windows::WindowRegistry;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::core::config::ActivityState;
use lyrel_core::core::program_status::{
    BlockedKind, ProgramRecord, ProgramSnapshot, ProgramState, ProgramStatusFrame,
};
use lyrel_terminal::view::TerminalView;
use lyrel_ui_base::config::Config;
use std::time::{Duration, Instant};

fn register(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    let weak = app.downgrade();
    app.update_in(vcx, |app, window, cx| {
        app.workspace = lyrel_ui_base::session::WorkspaceId::new();
        WindowRegistry::register(cx, app.workspace, window.window_handle(), weak);
    });
}

fn pane(app: &Entity<LyrelApp>, ix: usize, vcx: &mut VisualTestContext) -> Entity<TerminalView> {
    app.update(vcx, |app, _| app.tabs[ix].pane.terminals()[0].clone())
}

fn report(
    view: &Entity<TerminalView>,
    stream: &mut Stream,
    status: AgentStatus,
    outcome: Option<AgentOutcome>,
    unread: bool,
    vcx: &mut VisualTestContext,
) {
    let state = AgentSessionState {
        status,
        rich: true,
        turns: 1,
        readout: AgentReadout {
            result: outcome.map(|outcome| AgentResult {
                outcome,
                reason: None,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
        .encode(stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(state.clone())))
        .encode(stream)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.background_executor.run_until_parked();
        if view.read_with(vcx, |view, _| {
            view.agent_session() == Some(state.clone()) && view.agent().is_some()
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "agent report never reached the production TerminalView"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    if unread {
        view.update(vcx, |view, cx| view.mark_agent_result_unread(false, cx));
    }
}

/// Recompute and apply the aggregate outside any window lease, returning the
/// count the consumer read. Production does this from a deferred callback.
fn refresh(vcx: &mut VisualTestContext) -> usize {
    vcx.update(|_, cx| {
        SystemBadgeState::reconcile(cx);
        cx.global::<SystemBadgeState>().count()
    })
}

fn last_badge() -> Option<String> {
    take_test_badges().pop().flatten()
}

#[gpui_kit::test]
fn badge_aggregates_distinct_tabs_across_windows_and_dedupes_split_panes(cx: &mut TestAppContext) {
    let (a, mut av, mut ast) = harness_with_tabs(cx, 3);
    let (b, mut bv, mut bst) = harness_with_tabs(cx, 1);
    av.update(|_, cx| WindowRegistry::init(cx));
    register(&a, &mut av);
    register(&b, &mut bv);

    let a0 = pane(&a, 0, &mut av);
    let a1 = pane(&a, 1, &mut av);
    let a2 = pane(&a, 2, &mut av);
    let b0 = pane(&b, 0, &mut bv);
    // Two panes in one tab: both blocked, but the tab counts once.
    a.update_in(&mut av, |app, _, cx| {
        app.tabs.clear();
        app.tabs.push(crate::ui::tab::Tab::new(Pane::split_node(
            gpui_kit::Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Ready(a0.clone())),
            Pane::Leaf(PaneSlot::Ready(a1.clone())),
        )));
        app.tabs
            .push(crate::ui::tab::Tab::new(Pane::leaf(PaneSlot::Ready(
                a2.clone(),
            ))));
        app.tabs.select(0);
        cx.notify();
    });

    report(&a0, &mut ast[0], AgentStatus::Waiting, None, false, &mut av);
    report(&a1, &mut ast[1], AgentStatus::Waiting, None, false, &mut av);
    report(&a2, &mut ast[2], AgentStatus::Waiting, None, false, &mut av);
    report(&b0, &mut bst[0], AgentStatus::Waiting, None, false, &mut bv);

    assert_eq!(
        a.read_with(&av, |app, cx| app.badge_tab_count(cx)),
        2,
        "two qualified panes in one tab count once"
    );
    assert_eq!(b.read_with(&bv, |app, cx| app.badge_tab_count(cx)), 1);
    assert_eq!(refresh(&mut av), 3);
    assert_eq!(last_badge().as_deref(), Some("3"));
}

#[gpui_kit::test]
fn badge_defaults_cover_waiting_and_failed_but_not_ended(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    let view = pane(&app, 0, &mut vcx);

    report(
        &view,
        &mut streams[0],
        AgentStatus::Waiting,
        None,
        false,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 1);

    // A failed unread result stays counted through the Failed row.
    report(
        &view,
        &mut streams[0],
        AgentStatus::Done,
        Some(AgentOutcome::Failed),
        true,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 1);

    // Ended is off by default, so a successful unread result drops out.
    report(
        &view,
        &mut streams[0],
        AgentStatus::Done,
        Some(AgentOutcome::Succeeded),
        true,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 0);

    // The settings matrix can open Ended...
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().program_activity.ended.badge = true;
    });
    assert_eq!(refresh(&mut vcx), 1);

    // ...and closing Failed removes a failed result.
    vcx.update(|_, cx| {
        let activity = &mut cx.global_mut::<Config>().program_activity;
        activity.ended.badge = false;
        activity.failed.badge = false;
    });
    report(
        &view,
        &mut streams[0],
        AgentStatus::Done,
        Some(AgentOutcome::Failed),
        true,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 0);

    // Running has no badge channel: it never counts even with every row on.
    vcx.update(|_, cx| {
        let activity = &mut cx.global_mut::<Config>().program_activity;
        activity.waiting.badge = true;
        activity.ended.badge = true;
        activity.failed.badge = true;
    });
    report(
        &view,
        &mut streams[0],
        AgentStatus::Working,
        None,
        false,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 0);
}

#[gpui_kit::test]
fn zero_aggregate_clears_the_system_badge(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    let view = pane(&app, 0, &mut vcx);

    report(
        &view,
        &mut streams[0],
        AgentStatus::Waiting,
        None,
        false,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 1);
    assert_eq!(last_badge().as_deref(), Some("1"));

    report(
        &view,
        &mut streams[0],
        AgentStatus::Idle,
        None,
        false,
        &mut vcx,
    );
    assert_eq!(refresh(&mut vcx), 0);
    assert_eq!(
        last_badge(),
        None,
        "a zero aggregate clears the platform badge"
    );
}

#[gpui_kit::test]
fn program_records_count_blocked_waits_and_unread_failures(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    let view = pane(&app, 0, &mut vcx);
    let source = view.read_with(&*vcx, |view, _| view.program_source());

    let blocked = ProgramRecord::new(
        "build".into(),
        ProgramState::Blocked,
        Some(BlockedKind::Permission),
        None,
        None,
        None,
        None,
        1,
        1,
        1,
    );
    app.update(&mut vcx, |app, _| {
        app.program_attention.observe(
            source,
            &ProgramStatusFrame::Baseline(ProgramSnapshot::new(
                "session-1".into(),
                1,
                1,
                vec![blocked],
            )),
            false,
        );
    });
    assert_eq!(refresh(&mut vcx), 1);

    // Closing the Waiting badge removes the live blocker.
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().program_activity.waiting.badge = false;
    });
    assert_eq!(refresh(&mut vcx), 0);

    // An unread error counts through Failed even while Waiting is off.
    let error = ProgramRecord::new(
        "tests".into(),
        ProgramState::Error,
        None,
        None,
        None,
        None,
        None,
        2,
        2,
        2,
    );
    app.update(&mut vcx, |app, _| {
        app.program_attention.observe(
            source,
            &ProgramStatusFrame::Baseline(ProgramSnapshot::new(
                "session-1".into(),
                2,
                2,
                vec![error],
            )),
            false,
        );
    });
    assert_eq!(refresh(&mut vcx), 1);

    // Acknowledging the unread error clears it.
    app.update(&mut vcx, |app, _| {
        app.program_attention.acknowledge(source);
    });
    assert_eq!(refresh(&mut vcx), 0);
}

#[gpui_kit::test]
fn settings_badge_channel_controls_the_system_badge(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    let view = pane(&app, 0, &mut vcx);
    report(
        &view,
        &mut streams[0],
        AgentStatus::Waiting,
        None,
        false,
        &mut vcx,
    );

    let settings = crate::ui::settings::test_window::open(&app, &mut vcx);
    settings.update(&mut vcx, |settings, cx| {
        settings.set_activity_channel(ActivityState::Waiting, ActivityChannel::Badge, false, cx);
    });
    vcx.run_until_parked();
    assert!(
        !vcx.update(|_, cx| cx.global::<Config>().program_activity.waiting.badge),
        "the settings switch writes the badge channel"
    );
    assert_eq!(refresh(&mut vcx), 0);

    settings.update(&mut vcx, |settings, cx| {
        settings.set_activity_channel(ActivityState::Waiting, ActivityChannel::Badge, true, cx);
    });
    vcx.run_until_parked();
    assert_eq!(refresh(&mut vcx), 1);
}

#[test]
fn badge_label_caps_at_nine_plus() {
    assert_eq!(label_for(0), None);
    for count in 1..=9 {
        assert_eq!(label_for(count), Some(count.to_string()));
    }
    assert_eq!(label_for(10), Some("9+".to_owned()));
    assert_eq!(label_for(99), Some("9+".to_owned()));
}
