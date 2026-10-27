//! Consumer regressions for agent observations and read marks.

use super::*;
use super::gpui_tests::harness;
use crate::daemon::protocol::DaemonMsg;
use crate::daemon::transport::Stream;
use gpui_kit::{Entity, TestAppContext};

#[gpui_kit::test]
fn a_reported_session_id_asks_the_window_to_save(cx: &mut TestAppContext) {
    use crate::core::cli_agent::{AgentSessionState, AgentStatus};

    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    let saves = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    {
        let saves = saves.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, _: &AgentSessionChanged, _| {
                saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .detach();
        });
    }

    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status: AgentStatus::Idle,
        message: None,
        session_id: Some("sid-abc".into()),
        launch_argv: Some(vec!["omp".into()]),
        rich: true,
        cwd: None,
        activity: 0,
        turns: 0,
        pending_requests: Vec::new(),
        request_resume_status: Default::default(),
        pending_stop: false,
        readout: Default::default(),
    }))
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.agent_session().is_some())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            view.poll_agent_status(false, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        saves.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the id has to reach the layout on file"
    );

    window
        .update(cx, |view, window, cx| {
            view.poll_agent_status(false, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        saves.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an unchanged session must not re-save on every poll"
    );
}

/// Report a session in `status` on `daemon` and wait for `pane` to see it.
fn report_agent_status(
    status: crate::core::cli_agent::AgentStatus,
    pane: &gpui_kit::Entity<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    report_agent_turn(status, 0, pane, cx, daemon);
}

/// The same, for a session that has finished `turns` turns so far.
fn report_agent_turn(
    status: crate::core::cli_agent::AgentStatus,
    turns: u64,
    pane: &gpui_kit::Entity<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    use crate::core::cli_agent::AgentSessionState;

    let state = AgentSessionState {
        status,
        message: None,
        session_id: Some("sid-abc".into()),
        launch_argv: Some(vec!["omp".into()]),
        rich: true,
        cwd: None,
        activity: 0,
        turns,
        pending_requests: Vec::new(),
        request_resume_status: Default::default(),
        pending_stop: false,
        readout: Default::default(),
    };
    DaemonMsg::AgentStatus(Some(state.clone()))
        .encode(daemon)
        .unwrap();
    for _ in 0..200 {
        if cx.update(|cx| pane.read(cx).terminal.agent_session()) == Some(state.clone()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the agent status never reached the pane");
}

/// Poll `pane`'s agent status inside `window` and read its badge back.
fn poll_unread(
    window: gpui_kit::WindowHandle<TerminalView>,
    pane: &gpui_kit::Entity<TerminalView>,
    cx: &mut TestAppContext,
) -> bool {
    // Through the untyped handle: the typed one leases the root view, and
    // `pane` may be that view.
    cx.update_window(window.into(), |_, window, cx| {
        pane.update(cx, |pane, cx| {
            pane.poll_agent_status(false, window, cx);
            pane.agent_result_unread()
        })
    })
    .unwrap()
}

/// Switching workspaces, or reopening a window from the tray, throws the
/// pane's view away and builds a new one on the same daemon pane (#870).
/// The new view's first look at a `Done` agent is not a turn finishing —
/// the reader watched that one finish before the old view went.
#[gpui_kit::test]
fn a_rebuilt_pane_does_not_re_badge_a_turn_the_reader_already_saw(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &before, cx, &mut before_daemon);
    assert!(
        !poll_unread(window, &before, cx),
        "the reader watched it finish"
    );

    // The same daemon pane, rebuilt the way `SessionRestore::tabs` rebuilds it,
    // with the reader's focus somewhere else.
    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &after, cx, &mut daemon);
    assert!(
        !poll_unread(window, &after, cx),
        "rebuilding the pane is not a turn finishing"
    );
}

/// What the rebuild must not swallow: a turn that was still running when
/// the view went away and finished before the new one arrived.
#[gpui_kit::test]
fn a_turn_that_finished_while_the_pane_was_away_still_badges(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Working, 0, &before, cx, &mut before_daemon);
    assert!(!poll_unread(window, &before, cx));

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &after, cx, &mut daemon);
    assert!(
        poll_unread(window, &after, cx),
        "nobody saw this turn finish"
    );
}

/// Nor a whole later turn: the reader saw turn one, the agent was sent
/// another and finished it while the pane was away. The status reads
/// `Done` both times; only the turn count tells them apart.
#[gpui_kit::test]
fn a_later_turn_that_finished_while_the_pane_was_away_still_badges(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &before, cx, &mut before_daemon);
    assert!(!poll_unread(window, &before, cx));

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 2, &after, cx, &mut daemon);
    assert!(
        poll_unread(window, &after, cx),
        "the second turn finished unseen"
    );
}

/// And a badge the reader had not cleared yet comes back with the pane.
#[gpui_kit::test]
fn an_unread_turn_is_still_unread_after_the_pane_is_rebuilt(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    // Building another pane takes the window's focus off `before`.
    let (_elsewhere, _elsewhere_daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(5, window, cx))
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &before, cx, &mut before_daemon);
    assert!(poll_unread(window, &before, cx), "nobody was looking");

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &after, cx, &mut daemon);
    assert!(
        poll_unread(window, &after, cx),
        "rebuilding the pane is not reading it"
    );
}

/// The badge answers "did the reader see this?", so it has to read the
/// window's live focus rather than anything a focus callback left behind.
///
/// A second pane built into the same window is the case that goes wrong:
/// it holds no focus and was never told it lost any, so a flag a callback
/// maintains says whatever it was born saying — and the badge lands on
/// exactly the pane nobody is looking at.
#[gpui_kit::test]
fn a_finished_turn_on_an_unfocused_pane_is_unread(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    crate::core::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(2, window, cx))
        .unwrap();
    // Building a pane takes the window's focus (`with_terminal`), and the
    // reader's next click hands it back. The new pane is not in the element
    // tree, so nothing delivers it the blur that goes with losing it.
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            assert!(
                !pane.read(cx).focus_handle.is_focused(window),
                "the focus went back to the pane the reader is on"
            );
        })
        .unwrap();

    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(pane.agent_result_unread(), "nobody was looking at the pane");
            });
        })
        .unwrap();
}

/// Reinstalling or restarting the app leaves the daemon — and every agent
/// in it — running, so each restored tab reattaches to a pane whose agent
/// finished its turn long ago. The daemon replays that status, and reading
/// it as a turn that just landed put an unread badge on every agent tab in
/// the window the moment it opened.
#[gpui_kit::test]
fn a_restored_pane_does_not_badge_the_turn_it_reattached_to(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    crate::core::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(2, window, cx)
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(
                    !pane.agent_result_unread(),
                    "the replayed status is where this pane starts, not a result that \
                     just arrived"
                );
            });
        })
        .unwrap();

    // And the pane is still armed: the next turn it actually watches finish
    // badges exactly as it would have without the reattach.
    report_agent_status(AgentStatus::Working, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
            });
        })
        .unwrap();
    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(
                    pane.agent_result_unread(),
                    "a turn that finished while the reader was elsewhere is unread"
                );
            });
        })
        .unwrap();
}

/// Build `pane_id`'s first view (unfocused), let it watch `status` at
/// `turns`, then rebuild the pane the way restoring a workspace does — a
/// reattach, whose head is the daemon replaying `replayed` — and read the
/// rebuilt view's badge.
fn rebuild_by_reattach(
    watched: (crate::core::cli_agent::AgentStatus, u64),
    replayed: (crate::core::cli_agent::AgentStatus, u64),
    cx: &mut TestAppContext,
) -> bool {
    let (window, _root_daemon) = harness(cx);
    let focus_root = |cx: &mut TestAppContext| {
        window
            .update(cx, |view, window, cx| {
                view.focus_handle.clone().focus(window, cx)
            })
            .unwrap();
        cx.run_until_parked();
    };
    let (before, mut before_daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(2, window, cx))
        .unwrap();
    focus_root(cx);
    report_agent_turn(watched.0, watched.1, &before, cx, &mut before_daemon);
    poll_unread(window, &before, cx);
    drop(before);

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(2, window, cx)
        })
        .unwrap();
    focus_root(cx);
    report_agent_turn(replayed.0, replayed.1, &after, cx, &mut daemon);
    poll_unread(window, &after, cx)
}

/// A reattach whose pane this app already watched is a rebuild, not a
/// restart: the read mark, not the replay, says whether the turn is news.
#[gpui_kit::test]
fn a_reattached_pane_takes_back_the_badge_its_reader_left(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;
    assert!(
        rebuild_by_reattach((AgentStatus::Done, 1), (AgentStatus::Done, 1), cx),
        "the reader never cleared that badge; rebuilding the pane is not reading it"
    );
}

#[gpui_kit::test]
fn a_reattached_pane_badges_a_turn_that_was_running_when_its_view_went(
    cx: &mut TestAppContext,
) {
    use crate::core::cli_agent::AgentStatus;
    assert!(
        rebuild_by_reattach((AgentStatus::Working, 0), (AgentStatus::Done, 1), cx),
        "nobody saw this turn finish"
    );
}

#[gpui_kit::test]
fn a_reattached_pane_badges_a_later_turn_that_finished_while_away(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;
    // Turn one left a mark; turn two finishing before the reattach is a
    // different count, so the mark does not vouch for it.
    assert!(
        rebuild_by_reattach((AgentStatus::Done, 1), (AgentStatus::Done, 2), cx),
        "the second turn finished unseen"
    );
}

/// A relink keeps the view and what it last saw. A turn that was running
/// when the link dropped and finished before it came back reaches the view
/// only as the daemon's replay, and that replay has to badge: nobody saw
/// the turn finish.
#[gpui_kit::test]
fn a_turn_that_finished_while_the_link_was_down_still_badges(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    crate::core::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(2, window, cx)
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    report_agent_status(AgentStatus::Working, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(!pane.agent_result_unread(), "the turn is still running");
            });
        })
        .unwrap();

    let (new_client, mut new_daemon) = super::test_stream_pair();
    pane.update(cx, |pane, cx| {
        pane.adopt_relink(
            new_client,
            Vec::new(),
            &crate::terminal::PaneRoute::Local,
            TermSize::new(80, 24),
            8,
            17,
            cx,
        )
        .expect("the swap itself cannot fail");
    });
    drop(daemon);

    report_agent_status(AgentStatus::Done, &pane, cx, &mut new_daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(
                    pane.agent_result_unread(),
                    "the turn finished while the link was down, so nobody read it"
                );
            });
        })
        .unwrap();
}

#[gpui_kit::test]
fn a_finished_turn_on_the_focused_pane_is_already_read(cx: &mut TestAppContext) {
    use crate::core::cli_agent::AgentStatus;

    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    let pane = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            assert!(view.focus_handle.is_focused(window));
            view.poll_agent_status(false, window, cx);
            assert!(
                !view.agent_result_unread(),
                "the reader watched the turn finish"
            );
        })
        .unwrap();
}

/// Quick launch counts an agent each time one starts. Reattaching to a pane
/// whose agent was already running — every agent tab after an app restart
/// — is not a start, and counting it bumped every agent once per launch of
/// the app. Once that pane's agent exits, the next one is a start again.
#[gpui_kit::test]
fn only_an_agent_that_starts_under_this_view_counts_as_detected(cx: &mut TestAppContext) {
    use crate::core::cli_agent::CLIAgent;

    crate::core::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let detected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let pane = |reattached: bool, cx: &mut TestAppContext| {
        let (pane, daemon) = window
            .update(cx, |_, window, cx| {
                if reattached {
                    super::quiet_reattached_test_pane(2, window, cx)
                } else {
                    super::quiet_test_pane(3, window, cx)
                }
            })
            .unwrap();
        let seen = detected.clone();
        cx.update(|cx| {
            cx.subscribe(&pane, move |_, ev: &AgentDetected, _| {
                seen.borrow_mut().push(ev.0)
            })
            .detach()
        });
        (pane, daemon)
    };
    let report = |agent: Option<CLIAgent>,
                  at_prompt: bool,
                  pane: &Entity<TerminalView>,
                  daemon: &mut Stream,
                  cx: &mut TestAppContext| {
        DaemonMsg::Agent(agent).encode(daemon).unwrap();
        for _ in 0..200 {
            if pane.read_with(cx, |p, _| p.terminal.foreground_agent()) == agent {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        pane.update(cx, |p, cx| p.poll_agent_detection(at_prompt, cx));
        cx.run_until_parked();
    };

    let (restored, mut restored_daemon) = pane(true, cx);
    report(
        Some(CLIAgent::OhMyPi),
        false,
        &restored,
        &mut restored_daemon,
        cx,
    );
    report(
        Some(CLIAgent::OhMyPi),
        false,
        &restored,
        &mut restored_daemon,
        cx,
    );
    assert_eq!(
        *detected.borrow(),
        vec![],
        "already running when reattached"
    );
    report(None, true, &restored, &mut restored_daemon, cx);
    report(
        Some(CLIAgent::Pi),
        false,
        &restored,
        &mut restored_daemon,
        cx,
    );
    assert_eq!(
        *detected.borrow(),
        vec![CLIAgent::Pi],
        "started after the reattach"
    );

    let (fresh, mut fresh_daemon) = pane(false, cx);
    report(Some(CLIAgent::OhMyPi), false, &fresh, &mut fresh_daemon, cx);
    report(Some(CLIAgent::OhMyPi), false, &fresh, &mut fresh_daemon, cx);
    assert_eq!(
        *detected.borrow(),
        vec![CLIAgent::Pi, CLIAgent::OhMyPi],
        "a spawned pane counts its first agent, once"
    );
}
