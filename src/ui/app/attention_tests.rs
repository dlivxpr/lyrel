use super::test_window::harness_with_tabs;
use super::*;
use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::daemon::transport::Stream;
use crate::ui::windows::WindowRegistry;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::time::{Duration, Instant};

fn register(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    let weak = app.downgrade();
    app.update_in(vcx, |app, window, cx| {
        app.workspace = crate::core::session::WorkspaceId::new();
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
    since_ms: Option<u64>,
    unread: bool,
    vcx: &mut VisualTestContext,
) {
    let state = AgentSessionState {
        status,
        since_ms,
        rich: true,
        turns: 1,
        ..Default::default()
    };
    DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
        .encode(stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(state.clone()))
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
            "agent report never reached production TerminalView"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    if unread {
        view.update(vcx, |view, cx| view.mark_agent_result_unread(false, cx));
    }
}

fn assert_focused(app: &Entity<LyrelApp>, ix: usize, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| {
        assert_eq!(app.tabs.active_index(), ix);
        assert!(
            app.tabs[ix].pane.terminals()[0]
                .read(cx)
                .focus_handle
                .is_focused(window)
        );
    });
}

#[gpui_kit::test]
fn attention_keyboard_visits_oldest_waits_across_windows_then_unread_results(
    cx: &mut TestAppContext,
) {
    let (a, mut av, mut ast) = harness_with_tabs(cx, 3);
    let (b, mut bv, mut bst) = harness_with_tabs(cx, 2);
    crate::ui::i18n::set_locale("en");
    av.update(|_, cx| WindowRegistry::init(cx));
    register(&a, &mut av);
    register(&b, &mut bv);
    let a0 = pane(&a, 0, &mut av);
    let newer = pane(&a, 1, &mut av);
    let done = pane(&a, 2, &mut av);
    let oldest = pane(&b, 0, &mut bv);
    let unknown = pane(&b, 1, &mut bv);
    let now = crate::core::config::unix_now().saturating_mul(1000);
    report(
        &newer,
        &mut ast[1],
        AgentStatus::Waiting,
        Some(now.saturating_sub(60_000)),
        false,
        &mut av,
    );
    report(
        &done,
        &mut ast[2],
        AgentStatus::Done,
        Some(1),
        true,
        &mut av,
    );
    report(
        &oldest,
        &mut bst[0],
        AgentStatus::Waiting,
        Some(now.saturating_sub(180_000)),
        false,
        &mut bv,
    );
    report(
        &unknown,
        &mut bst[1],
        AgentStatus::Waiting,
        None,
        false,
        &mut bv,
    );
    a.update_in(&mut av, |app, window, cx| {
        app.tabs.select(0);
        a0.read(cx).focus_handle.focus(window, cx);
    });
    av.background_executor.run_until_parked();

    av.simulate_keystrokes("secondary-shift-u");
    bv.background_executor.run_until_parked();
    assert_focused(&b, 0, &mut bv);
    bv.simulate_keystrokes("secondary-shift-u");
    av.background_executor.run_until_parked();
    assert_focused(&a, 1, &mut av);
    av.simulate_keystrokes("secondary-shift-u");
    bv.background_executor.run_until_parked();
    assert_focused(&b, 1, &mut bv);
    bv.simulate_keystrokes("secondary-shift-u");
    av.background_executor.run_until_parked();
    assert_focused(&a, 2, &mut av);
    assert!(
        !done.read_with(&av, |view, _| view.agent_result_unread()),
        "revealing a completed result marks it read"
    );
    av.simulate_keystrokes("secondary-shift-u");
    bv.background_executor.run_until_parked();
    assert_focused(&b, 0, &mut bv);
    assert_eq!(a.read_with(&av, |app, _| app.tabs.len()), 3);
    assert_eq!(b.read_with(&bv, |app, _| app.tabs.len()), 2);

    // Both consumers project the same daemon-owned spell; absence stays unknown.
    b.update(&mut bv, |app, cx| {
        let rows = app.agent_rows(cx);
        assert_eq!(rows[0].waited.as_deref(), Some("waiting 3m"));
        assert_eq!(rows[1].waited, None);
        let tabs = app.tab_rows_for(app.workspace, false, cx);
        assert_eq!(tabs[0].waited, rows[0].waited);
        assert_eq!(tabs[1].waited, None);
        assert_eq!(rows[1].since_ms, None);
    });
    report(
        &unknown,
        &mut bst[1],
        AgentStatus::Waiting,
        Some(u64::MAX),
        false,
        &mut bv,
    );
    b.update(&mut bv, |app, cx| {
        assert_eq!(app.agent_rows(cx)[1].waited.as_deref(), Some("waiting <1m"));
    });
}

#[gpui_kit::test]
fn attention_binding_preserves_user_chords_and_plain_terminal_control_u(cx: &mut TestAppContext) {
    use crate::core::config::KeybindingOverride;
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    let first = pane(&app, 0, &mut vcx);
    let waiting = pane(&app, 1, &mut vcx);
    report(
        &waiting,
        &mut streams[1],
        AgentStatus::Waiting,
        Some(1),
        false,
        &mut vcx,
    );
    app.update_in(&mut vcx, |_, window, cx| {
        first.read(cx).focus_handle.focus(window, cx)
    });
    vcx.background_executor.run_until_parked();
    vcx.simulate_keystrokes("ctrl-u");
    streams[0]
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    loop {
        match ClientMsg::read(&mut streams[0]) {
            Ok(ClientMsg::Input(bytes)) => {
                assert_eq!(bytes, b"\x15");
                break;
            }
            Ok(_) => {}
            Err(error) => panic!("plain Ctrl+U never reached the terminal: {error}"),
        }
    }
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().keybindings.insert(
            "NextTab".into(),
            KeybindingOverride::Add("secondary-shift-u".into()),
        );
        crate::ui::keymap::rebind(cx);
    });
    vcx.simulate_keystrokes("secondary-shift-u");
    assert_focused(&app, 1, &mut vcx);
    // NextTab wraps to the ordinary pane; the attention action would stay on
    // the sole waiting pane, so this distinguishes the configured winner.
    vcx.simulate_keystrokes("secondary-shift-u");
    assert_focused(&app, 0, &mut vcx);
    vcx.update(|_, cx| {
        let cfg = cx.global_mut::<Config>();
        cfg.keybindings.remove("NextTab");
        cfg.keybindings.insert(
            "FocusNextAgentNeedingAttention".into(),
            KeybindingOverride::Exact(vec!["f6".into()]),
        );
        crate::ui::keymap::rebind(cx);
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.tabs.select(0);
        first.read(cx).focus_handle.focus(window, cx);
    });
    vcx.background_executor.run_until_parked();
    vcx.simulate_keystrokes("f6");
    assert_focused(&app, 1, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.tabs.select(0);
        first.read(cx).focus_handle.focus(window, cx);
        app.open_search(SearchTab::Actions, "", window, cx);
    });
    vcx.run_until_parked();
    vcx.simulate_input("focus-next-agent-needing-attention");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_focused(&app, 1, &mut vcx);
    assert!(!app.read_with(&vcx, |app, _| app.search.is_open()));
}
