//! Consumer tests for the status bar's Caffeine control (#379).
//!
//! The seam is the production workbench window: a real status bar, a real
//! popup menu, real daemon-stream agent reports (disposable agents — no real
//! CLI install), and the real native power request underneath the app-owned
//! controller. Assertions read the shared [`CaffeineState`] truth (mode,
//! live hold, error), the menu's checked item and footer text, and the
//! waiting badge — the same surface the user sees.

use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, MouseButton, TestAppContext, VisualTestContext};
use lyrel_ui_base::i18n::{L10nKey, set_locale, t};
use lyrel_ui_base::session::{
    RemoteRef, RemoteTarget, WindowView, WindowViews, WorkspaceId, WorkspaceStore,
};
use std::time::{Duration, Instant};

use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use crate::daemon::protocol::DaemonMsg;
use crate::daemon::transport::Stream;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::caffeine::{CaffeineMode, CaffeineState};
use crate::ui::windows::WindowRegistry;

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

fn register(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    let weak = app.downgrade();
    app.update_in(vcx, |app, window, cx| {
        app.workspace = WorkspaceId::new();
        WindowRegistry::register(cx, app.workspace, window.window_handle(), weak);
    });
}

/// The shared truth every window's cup projects.
fn caffeine_truth(vcx: &mut VisualTestContext) -> (CaffeineMode, bool, Option<String>) {
    vcx.update(|_, cx| {
        let state = cx.global::<CaffeineState>();
        (
            state.mode(),
            state.holding(),
            state.last_error().map(str::to_owned),
        )
    })
}

fn open_menu(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.click("status-caffeine", cx));
    settle(vcx);
}

fn click_label(vcx: &mut VisualTestContext, label: &str) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(label);
        let center = item.bounds().center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    });
    settle(vcx);
}

/// The checked marker the menu paints on its current mode.
fn menu_checked(vcx: &mut VisualTestContext, label: &str) -> Option<bool> {
    vcx.update(|window, _| {
        window
            .try_find_by_label(label)
            .and_then(|item| item.checked())
    })
}

fn footer_text(vcx: &mut VisualTestContext) -> Option<String> {
    let candidates = [
        t(L10nKey::StatusCaffeineFooterOff).to_string(),
        t(L10nKey::StatusCaffeineFooterHolding).to_string(),
        t(L10nKey::StatusCaffeineFooterWaiting).to_string(),
        cx_error_footer(vcx),
    ];
    vcx.update(|window, _| {
        candidates
            .iter()
            .find(|text| window.try_find_by_label(text.as_str()).is_some())
            .cloned()
    })
}

fn cx_error_footer(vcx: &mut VisualTestContext) -> String {
    let (_, _, error) = caffeine_truth(vcx);
    lyrel_ui_base::i18n::t_fmt(
        L10nKey::StatusCaffeineFooterError,
        &[("error", error.as_deref().unwrap_or(""))],
    )
}

fn report(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    stream: &mut Stream,
    status: AgentStatus,
) {
    let view = app.read_with(vcx, |app, _| app.tabs[0].pane.terminals()[0].clone());
    report_on(&view, stream, status, vcx);
}

fn report_on(
    view: &gpui_kit::Entity<lyrel_terminal::view::TerminalView>,
    stream: &mut Stream,
    status: AgentStatus,
    vcx: &mut VisualTestContext,
) {
    let state = AgentSessionState {
        status,
        rich: true,
        turns: 1,
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
            "agent report never reached the production pane"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // The stream landing is not the whole pump: the foreground poll is what
    // turns a session change into the pane's AgentSessionChanged event, the
    // same poll the production pump runs on every wakeup.
    vcx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.poll_foreground(window, cx);
        });
    });
}

#[gpui_kit::test]
fn menu_offers_three_modes_and_reflects_the_live_hold(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, _stream) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    settle(&mut vcx);

    vcx.update(|window, _| {
        assert!(window.find("status-caffeine").visible());
        assert!(
            window.try_find("status-caffeine-waiting-dot").is_none(),
            "Off has no waiting badge"
        );
    });

    open_menu(&mut vcx);
    for label in [
        t(L10nKey::StatusCaffeineModeOff),
        t(L10nKey::StatusCaffeineModeAlwaysOn),
        t(L10nKey::StatusCaffeineModeWhileAgentsWork),
    ] {
        assert!(
            vcx.update(|window, _| window.try_find_by_label(label).is_some()),
            "menu must offer {label:?}"
        );
    }
    // The cup's accessible name carries the mode and the actual state; the
    // menu's title row keeps the plain title beside it (#401).
    let cup_label = vcx.update(|window, _| {
        window.find("status-caffeine").label().map(str::to_owned)
    });
    assert_eq!(
        cup_label.as_deref(),
        Some("Caffeine, Off: Sleeps per system power settings"),
        "Off exposes its mode and actual state in the accessible name"
    );
    assert!(
        vcx.update(|window, _| {
            window
                .find_all_by_label(t(L10nKey::StatusCaffeineTitle))
                .len()
                >= 1
        }),
        "the menu title shows beside the cup's accessible name"
    );
    assert_eq!(
        menu_checked(&mut vcx, t(L10nKey::StatusCaffeineModeOff)),
        Some(true),
        "the desktop starts in Off"
    );
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(t(L10nKey::StatusCaffeineFooterOff))
    );
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    settle(&mut vcx);

    // Always On acquires the real native request and says so.
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::AlwaysOn, true, None),
        "Always On holds the idle-sleep request"
    );
    open_menu(&mut vcx);
    assert_eq!(
        menu_checked(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn)),
        Some(true)
    );
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(t(L10nKey::StatusCaffeineFooterHolding))
    );
    vcx.update(|window, _| {
        assert!(
            window.try_find("status-caffeine-waiting-dot").is_none(),
            "a live hold is not a wait"
        );
    });
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    settle(&mut vcx);

    // Automatic mode with nothing working waits, visibly.
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None)
    );
    open_menu(&mut vcx);
    assert_eq!(
        menu_checked(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork)),
        Some(true)
    );
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(t(L10nKey::StatusCaffeineFooterWaiting))
    );
    vcx.update(|window, _| {
        assert!(
            window.find("status-caffeine-waiting-dot").visible(),
            "automatic mode paints the waiting badge while no agent works"
        );
    });
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    settle(&mut vcx);

    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    assert_eq!(caffeine_truth(&mut vcx), (CaffeineMode::Off, false, None));
    open_menu(&mut vcx);
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(t(L10nKey::StatusCaffeineFooterOff))
    );
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    settle(&mut vcx);
}

#[gpui_kit::test]
fn automatic_mode_holds_while_a_working_agent_runs_and_releases(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None)
    );

    report(&app, &mut vcx, &mut streams[0], AgentStatus::Working);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, true, None),
        "a Working agent holds the local computer awake"
    );

    report(&app, &mut vcx, &mut streams[0], AgentStatus::Waiting);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None),
        "waiting for input releases the request"
    );

    report(&app, &mut vcx, &mut streams[0], AgentStatus::Done);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None)
    );
}

#[gpui_kit::test]
fn one_mode_across_windows_and_attached_remote_agents_hold_this_machine(cx: &mut TestAppContext) {
    set_locale("en");
    let (a, mut av, _ast) = harness_with_tabs(cx, 1);
    let (b, mut bv, _bst) = harness_with_tabs(cx, 1);
    av.update(|_, cx| WindowRegistry::init(cx));
    register(&a, &mut av);

    // Window B is an attached remote workspace; its pane is an attached
    // (SSH) pane, exactly the shape a working remote agent reports through.
    let remote_ws = WorkspaceId::new();
    let remote = WindowView::on_remote(RemoteRef::new(
        RemoteTarget::Alias {
            alias: "caffeine-remote".into(),
        },
        remote_ws,
    ));
    bv.update(|_, cx| {
        let mut views = WorkspaceStore::all(cx).clone();
        views.views.push(remote);
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: views.views,
                active: None,
            },
        );
    });
    let weak_b = b.downgrade();
    b.update_in(&mut bv, |app, window, cx| {
        app.workspace = remote_ws;
        WindowRegistry::register(cx, remote_ws, window.window_handle(), weak_b);
    });
    let mut b_stream = b.update_in(&mut bv, |app, window, cx| {
        let (view, stream) = lyrel_terminal::view::quiet_test_ssh_pane(9, window, cx);
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs[0] = crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
            crate::ui::pane::PaneSlot::Ready(view),
        ));
        app.tabs.select(0);
        cx.notify();
        stream
    });

    // The mode chosen in window A is the mode window B shows.
    open_menu(&mut av);
    click_label(&mut av, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    open_menu(&mut bv);
    assert_eq!(
        menu_checked(&mut bv, t(L10nKey::StatusCaffeineModeWhileAgentsWork)),
        Some(true),
        "both windows share the one process-wide mode"
    );
    click_label(&mut bv, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    settle(&mut av);
    settle(&mut bv);

    // A Working agent on the attached remote host holds this computer.
    report(&b, &mut bv, &mut b_stream, AgentStatus::Working);
    settle(&mut bv);
    settle(&mut av);
    assert_eq!(
        caffeine_truth(&mut av),
        (CaffeineMode::WhileAgentsWork, true, None),
        "a remote working agent keeps the local desktop awake"
    );
    open_menu(&mut av);
    assert_eq!(
        footer_text(&mut av).as_deref(),
        Some(t(L10nKey::StatusCaffeineFooterHolding))
    );
    click_label(&mut av, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    settle(&mut av);

    report(&b, &mut bv, &mut b_stream, AgentStatus::Waiting);
    settle(&mut bv);
    settle(&mut av);
    assert_eq!(
        caffeine_truth(&mut av),
        (CaffeineMode::WhileAgentsWork, false, None)
    );
    av.update(|window, _| {
        assert!(window.find("status-caffeine-waiting-dot").visible());
    });
}

#[gpui_kit::test]
fn closing_the_agents_window_releases_automatic_mode(cx: &mut TestAppContext) {
    set_locale("en");
    let (a, mut av, _ast) = harness_with_tabs(cx, 1);
    let (b, mut bv, mut bst) = harness_with_tabs(cx, 1);
    av.update(|_, cx| WindowRegistry::init(cx));
    register(&a, &mut av);
    register(&b, &mut bv);
    let b_ws = b.read_with(&bv, |app, _| app.workspace);

    open_menu(&mut av);
    click_label(&mut av, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    report(&b, &mut bv, &mut bst[0], AgentStatus::Working);
    settle(&mut bv);
    settle(&mut av);
    assert_eq!(
        caffeine_truth(&mut av),
        (CaffeineMode::WhileAgentsWork, true, None)
    );

    // The window holding the only Working agent goes away; the app may stay
    // alive behind the tray, and the request must not.
    bv.update(|_, cx| WindowRegistry::unregister(cx, b_ws));
    av.update(|_, cx| CaffeineState::agents_changed(cx));
    settle(&mut av);
    assert_eq!(
        caffeine_truth(&mut av),
        (CaffeineMode::WhileAgentsWork, false, None),
        "no window with a Working agent means no hold"
    );
}

#[gpui_kit::test]
fn hiding_caffeine_in_settings_releases_the_hold_and_restores_off(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, _stream) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::AlwaysOn, true, None)
    );

    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(900.)));
    let toggle = |app: &Entity<LyrelApp>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, window, cx| {
            app.open_settings_section(crate::ui::settings::SettingsSection::Toolbar, window, cx);
        });
        settle(vcx);
        vcx.update(|window, cx| window.click("toolbar-caffeine", cx));
        settle(vcx);
        let settings = app.read_with(vcx, |app, _| app.settings.clone().unwrap());
        settings.update_in(vcx, |settings, window, cx| {
            settings.close_settings(window, cx)
        });
        settle(vcx);
    };
    toggle(&app, &mut vcx);
    vcx.update(|window, _| assert!(window.try_find("status-caffeine").is_none()));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::Off, false, None),
        "hiding the cup releases the request and returns the mode to Off"
    );
    toggle(&app, &mut vcx);
    open_menu(&mut vcx);
    assert_eq!(
        menu_checked(&mut vcx, t(L10nKey::StatusCaffeineModeOff)),
        Some(true)
    );
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(t(L10nKey::StatusCaffeineFooterOff))
    );
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    settle(&mut vcx);
}

#[gpui_kit::test]
fn acquire_and_release_failures_are_shown_not_hidden(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, _stream) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);

    // An acquire failure is the visible state; nothing is held.
    crate::ui::caffeine::force_failure_for_test(true);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    let (_, holding, error) = caffeine_truth(&mut vcx);
    assert!(!holding, "a failed acquire must not hold");
    assert_eq!(error.as_deref(), Some("injected power-API failure"));
    open_menu(&mut vcx);
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(
            lyrel_ui_base::i18n::t_fmt(
                L10nKey::StatusCaffeineFooterError,
                &[("error", "injected power-API failure")]
            )
            .as_str()
        ),
        "the menu tells the acquire failure's truth"
    );
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    settle(&mut vcx);
    crate::ui::caffeine::force_failure_for_test(false);

    // Stopping the mode after an acquire failure clears the stale error with it.
    open_menu(&mut vcx);
    crate::ui::caffeine::force_failure_for_test(true);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert!(caffeine_truth(&mut vcx).2.is_some(), "acquire failed again");
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::Off, false, None),
        "stopping the mode retires the acquire failure it left behind"
    );

    // A release failure keeps the hold live and says so.
    open_menu(&mut vcx);
    crate::ui::caffeine::force_failure_for_test(false);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::AlwaysOn, true, None)
    );
    crate::ui::caffeine::force_failure_for_test(true);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    let (mode, holding, error) = caffeine_truth(&mut vcx);
    assert_eq!(mode, CaffeineMode::Off, "the user's choice still stands");
    assert!(holding, "the still-live request is not pretended away");
    assert_eq!(error.as_deref(), Some("injected power-API failure"));
    open_menu(&mut vcx);
    assert_eq!(
        footer_text(&mut vcx).as_deref(),
        Some(
            lyrel_ui_base::i18n::t_fmt(
                L10nKey::StatusCaffeineFooterError,
                &[("error", "injected power-API failure")]
            )
            .as_str()
        )
    );
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    settle(&mut vcx);

    // The next real transition retries and recovers.
    crate::ui::caffeine::force_failure_for_test(false);
    vcx.update(|_, cx| CaffeineState::reset(cx));
    assert_eq!(caffeine_truth(&mut vcx), (CaffeineMode::Off, false, None));
}

#[gpui_kit::test]
fn waiting_pane_does_not_mask_a_working_pane_in_one_tab(cx: &mut TestAppContext) {
    use crate::ui::pane::{Pane, PaneSlot};
    set_locale("en");
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    // Both harness panes live in one tab as a split.
    let (left, right) = app.update_in(&mut vcx, |app, _, cx| {
        let left = app.tabs[0].pane.terminals()[0].clone();
        let right = app.tabs[1].pane.terminals()[0].clone();
        app.tabs.remove(1);
        app.tabs[0] = crate::ui::tab::Tab::new(Pane::split_node(
            gpui_kit::Axis::Horizontal,
            0.5,
            Pane::leaf(PaneSlot::Ready(left.clone())),
            Pane::leaf(PaneSlot::Ready(right.clone())),
        ));
        app.tabs.select(0);
        cx.notify();
        (left, right)
    });

    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    report_on(&left, &mut streams[0], AgentStatus::Waiting, &mut vcx);
    report_on(&right, &mut streams[1], AgentStatus::Working, &mut vcx);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, true, None),
        "the badge ranks Waiting above Working, but the hold must not"
    );

    report_on(&right, &mut streams[1], AgentStatus::Waiting, &mut vcx);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None)
    );
}

#[gpui_kit::test]
fn a_disconnected_pane_keeps_no_hold(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    report(&app, &mut vcx, &mut streams[0], AgentStatus::Working);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, true, None)
    );

    // A hard route drop leaves the last mirrored state behind; an exited pane
    // is not working and must not keep this computer awake.
    app.update_in(&mut vcx, |app, _, cx| {
        let pane = app.tabs[0].pane.terminals()[0].clone();
        pane.update(cx, |view, _| view.terminal.exited = true);
    });
    vcx.update(|_, cx| CaffeineState::agents_changed(cx));
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None),
        "the stale Working reading of an exited pane holds nothing"
    );
}

#[gpui_kit::test]
fn closing_the_working_tab_releases_automatic_mode(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    vcx.update(|_, cx| {
        cx.global_mut::<lyrel_ui_base::config::Config>()
            .confirm_close = lyrel_core::core::config::ConfirmClose::Never;
    });
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    report(&app, &mut vcx, &mut streams[0], AgentStatus::Working);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, true, None)
    );

    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, false, None),
        "closing the tab that held the Working agent releases the request"
    );
}

#[gpui_kit::test]
fn quitting_the_desktop_releases_the_request(cx: &mut TestAppContext) {
    set_locale("en");
    let (app, mut vcx, _stream) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::AlwaysOn, true, None)
    );

    // The path `on_app_quit` takes: the request's lifetime ends with the app,
    // and the mode goes Off so nothing can re-acquire during teardown.
    vcx.update(|_, cx| CaffeineState::release_for_quit(cx));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::Off, false, None),
        "exit releases the idle-sleep request"
    );
    // A late agent-status effect after exit must not bring the request back.
    vcx.update(|_, cx| CaffeineState::agents_changed(cx));
    settle(&mut vcx);
    assert_eq!(caffeine_truth(&mut vcx), (CaffeineMode::Off, false, None));
}

#[test]
fn cup_look_decides_the_four_presentations() {
    use crate::ui::caffeine::{cup_look, CupInk};

    // A live hold is the filled cup in the contrast-checked accent, whatever
    // a stale release error still says: the request is genuinely held and the
    // failure already shows in the menu footer and the accessible name.
    assert_eq!(
        cup_look(true, false),
        ("icons/coffee-filled.svg", CupInk::AccentInk),
        "the active hold fills the cup"
    );
    assert_eq!(
        cup_look(true, true),
        ("icons/coffee-filled.svg", CupInk::AccentInk),
        "a still-live request is not painted as a failure"
    );
    // Off and automatic waiting keep the muted outline...
    assert_eq!(
        cup_look(false, false),
        ("icons/coffee.svg", CupInk::Muted),
        "Off and the wait keep the outline in the caption role"
    );
    // ...failure keeps the outline with the danger role.
    assert_eq!(
        cup_look(false, true),
        ("icons/coffee.svg", CupInk::Danger),
        "a failed request keeps the outline and takes the danger role"
    );
}

#[gpui_kit::test]
fn the_accessible_name_exposes_mode_and_actual_state_through_the_four_states(
    cx: &mut TestAppContext,
) {
    set_locale("en");
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    register(&app, &mut vcx);

    let cup_name = |vcx: &mut VisualTestContext| -> Option<String> {
        vcx.update(|window, _| window.find("status-caffeine").label().map(str::to_owned))
    };

    // Off: the mode and the actual state, not the mode alone.
    assert_eq!(
        cup_name(&mut vcx).as_deref(),
        Some("Caffeine, Off: Sleeps per system power settings")
    );

    // Always On, held.
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::AlwaysOn, true, None)
    );
    assert_eq!(
        cup_name(&mut vcx).as_deref(),
        Some("Caffeine, Always On: Keeping this computer awake")
    );

    // Automatic mode, waiting on an agent.
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeWhileAgentsWork));
    assert_eq!(
        cup_name(&mut vcx).as_deref(),
        Some("Caffeine, While Agents Work: Waiting for an agent to start working")
    );

    // Automatic mode, actually holding while an agent works.
    report(&app, &mut vcx, &mut streams[0], AgentStatus::Working);
    settle(&mut vcx);
    assert_eq!(
        caffeine_truth(&mut vcx),
        (CaffeineMode::WhileAgentsWork, true, None)
    );
    assert_eq!(
        cup_name(&mut vcx).as_deref(),
        Some("Caffeine, While Agents Work: Keeping this computer awake"),
        "the actual hold replaces the wait in the accessible name"
    );
    report(&app, &mut vcx, &mut streams[0], AgentStatus::Done);
    settle(&mut vcx);

    // Failure: the accessible name carries the explanation itself.
    crate::ui::caffeine::force_failure_for_test(true);
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeAlwaysOn));
    assert!(caffeine_truth(&mut vcx).2.is_some());
    assert_eq!(
        cup_name(&mut vcx).as_deref(),
        Some("Caffeine, Always On: Couldn't keep the computer awake: injected power-API failure"),
        "the failure's explanation is part of the accessible name"
    );
    open_menu(&mut vcx);
    click_label(&mut vcx, t(L10nKey::StatusCaffeineModeOff));
    settle(&mut vcx);
    crate::ui::caffeine::force_failure_for_test(false);
    assert_eq!(
        cup_name(&mut vcx).as_deref(),
        Some("Caffeine, Off: Sleeps per system power settings"),
        "recovery restores the plain Off name"
    );
}

#[gpui_kit::test]
fn the_installed_theme_carries_the_contrast_checked_accent_ink(cx: &mut TestAppContext) {
    set_locale("en");
    let (_app, mut vcx, _stream) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| WindowRegistry::init(cx));
    settle(&mut vcx);

    // The role `render_caffeine` paints a live hold with is installed with
    // the preset, cleared to the text floor on the surfaces the status bar
    // can sit on — the same guarantee the theme-token tests pin for every
    // builtin theme.
    let (installed, background) = vcx.update(|_, cx| {
        let installed = cx.global::<lyrel_ui_base::presets::ActiveAccentInk>().0;
        let theme = lyrel_ui_base::presets::by_id(cx, lyrel_ui_base::presets::DEFAULT_ID);
        (installed, theme.neutrals().background)
    });
    let expected = lyrel_ui_base::presets::builtins()
        .into_iter()
        .find(|t| t.id == lyrel_ui_base::presets::DEFAULT_ID)
        .unwrap()
        .neutrals()
        .accent_ink;
    assert_eq!(installed, expected, "the live theme carries the token role");
    assert!(
        lyrel_ui_base::presets::contrast(installed, background) >= 4.5 - 0.01,
        "the installed accent ink clears the text floor on the window: {:.2}:1",
        lyrel_ui_base::presets::contrast(installed, background),
    );
}
