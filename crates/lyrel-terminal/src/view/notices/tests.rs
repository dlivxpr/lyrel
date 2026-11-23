use super::*;
use crate::view::test_support::*;
use gpui_kit::TestAppContext;
use lyrel_ui_base::notification_history::Severity;

/// What the label ladder asks a pane: are you showing a name of your own,
/// or still standing under the app's? (#740)
#[test]
fn a_pane_states_a_title_whenever_it_is_not_the_placeholder() {
    use crate::view::stated_title;

    // Nothing has spoken — this is the pane a directory stands in for.
    assert_eq!(stated_title("lyrel"), None);
    assert_eq!(
        stated_title("  lyrel  "),
        Some("  lyrel  "),
        "a program that set exactly this introduced itself; its spaces are payload"
    );
    assert_eq!(
        stated_title("   "),
        Some("   "),
        "whitespace is not the empty reset"
    );

    // A title from the program running in it.
    assert_eq!(stated_title("vim — main.rs"), Some("vim — main.rs"));
    assert_eq!(
        stated_title(" user@host:~/repo "),
        Some(" user@host:~/repo ")
    );
    // A default lyrel chose for the pane itself is a name, not the absence
    // of one: an SSH pane answers to its host (#438) and a workspace pane
    // to its workspace, and neither gives way to a directory.
    assert_eq!(stated_title("prod-web"), Some("prod-web"));
    // So does the state a finished pane is left showing.
    assert_eq!(
        stated_title("lyrel — process exited"),
        Some("lyrel — process exited")
    );
}

#[test]
fn a_title_the_tab_already_shows_drops_the_one_that_was_waiting() {
    // The revert is what cancels a flash: the pane is told the title it is
    // already displaying, and the queued command name goes away with it.
    assert_eq!(
        settle_title("~/dev", true, "~/dev"),
        TitleSettle::Revert,
        "a revert cancels the pending title rather than queueing behind it"
    );
    assert_eq!(
        settle_title("~/dev", false, "~/dev"),
        TitleSettle::Revert,
        "and asks for no wait when there was nothing pending either"
    );
    // A second title while one is already waiting rides the wait in
    // flight; restarting it is what would let a title that keeps changing
    // put the tab's next update off forever.
    assert_eq!(settle_title("~/dev", true, "wget 40%"), TitleSettle::Queue);
    assert_eq!(
        settle_title("~/dev", false, "wget 1%"),
        TitleSettle::QueueAndWait
    );
}

#[test]
fn a_spinner_frame_that_only_changes_the_status_mark_asks_for_no_repaint() {
    // An agent rewrites its title every few hundred milliseconds while it
    // works, and the tab never draws the mark, so none of these may wait
    // and repaint the whole window for the same label.
    assert_eq!(
        settle_title(
            "\u{25D0} fixing the switcher",
            false,
            "\u{25D1} fixing the switcher"
        ),
        TitleSettle::Revert
    );
    assert_eq!(
        settle_title(
            "\u{25D0} fixing the switcher",
            true,
            "\u{2733} fixing the switcher"
        ),
        TitleSettle::Revert,
        "and a frame that lands while another title waits drops it, as a revert does"
    );
    assert_eq!(
        settle_title(
            "\u{25D0} fixing the switcher",
            false,
            "\u{25D1} fixing the parser"
        ),
        TitleSettle::QueueAndWait,
        "a different title still lands"
    );
}

#[test]
fn a_notification_title_keeps_at_most_two_segments() {
    let ws = || Some("lyrel".to_string());
    assert_eq!(
        compose_notification_title(None, Some("build-box".into()), ws()),
        "build-box · lyrel"
    );
    // An agent takes the machine's place rather than adding a third part.
    assert_eq!(
        compose_notification_title(Some("Claude".into()), Some("build-box".into()), ws()),
        "Claude · lyrel"
    );
    // A local pane has no machine label; a nameless workspace has no name.
    assert_eq!(compose_notification_title(None, None, ws()), "lyrel");
    assert_eq!(
        compose_notification_title(Some("Claude".into()), None, None),
        "Claude"
    );
    assert_eq!(compose_notification_title(None, None, None), "lyrel");
}

/// Runs out the wait a new title is held for.
fn settle(cx: &mut TestAppContext) {
    cx.executor().advance_clock(TITLE_SETTLE * 2);
    cx.run_until_parked();
}
fn configure_notifications(
    cx: &mut TestAppContext,
    mode: lyrel_core::core::config::NotifyMode,
    threshold_secs: u64,
) {
    cx.update(|cx| {
        let mut config = cx.global::<lyrel_ui_base::config::Config>().clone();
        config.notify_on_command_finish = mode;
        config.notify_threshold_secs = threshold_secs;
        cx.set_global(config);
    });
}

fn report_prompt(
    daemon: &mut lyrel_core::daemon::transport::Stream,
    at_prompt: bool,
    last_exit: Option<i32>,
) {
    lyrel_core::daemon::protocol::DaemonMsg::Prompt {
        active: true,
        at_prompt,
        last_exit,
    }
    .encode(daemon)
    .unwrap();
}

fn wait_for_shell(
    window: &gpui_kit::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
    ready: impl Fn(&TerminalView) -> bool,
    what: &str,
) {
    for _ in 0..400 {
        if window.update(cx, |view, _, _| ready(view)).unwrap() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

/// Invoke the production foreground lifecycle instead of touching the view's
/// completion fields. It owns both the command lifecycle and notification
/// policy.
fn pump_foreground(cx: &mut TestAppContext) {
    // Let the lifecycle task reach its first timer before moving the clock.
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(310));
    cx.run_until_parked();
}

/// One ordinary command through the ordered daemon stream: at a prompt, leaving
/// it, then back with the shell's result.
fn ordinary_command(
    window: &gpui_kit::WindowHandle<TerminalView>,
    daemon: &mut lyrel_core::daemon::transport::Stream,
    cx: &mut TestAppContext,
    exit: Option<i32>,
) {
    report_prompt(daemon, true, None);
    wait_for_shell(window, cx, |view| view.terminal.at_prompt(), "a prompt");
    report_prompt(daemon, false, None);
    wait_for_shell(
        window,
        cx,
        |view| !view.terminal.at_prompt(),
        "the command to start",
    );
    pump_foreground(cx);
    report_prompt(daemon, true, exit);
    wait_for_shell(
        window,
        cx,
        |view| view.terminal.at_prompt() && view.terminal.last_exit_code() == exit,
        "the command result",
    );
    pump_foreground(cx);
}

fn take_notifications() -> Vec<crate::desktop_notification::DeliveredNotification> {
    crate::desktop_notification::take_test_notifications()
}

#[gpui_kit::test]
fn ordinary_command_notifications_keep_the_existing_threshold_and_mode(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, mut daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");

    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 3600);
    ordinary_command(&window, &mut daemon, cx, Some(0));
    assert!(
        take_notifications().is_empty(),
        "a command under the configured threshold stays quiet"
    );

    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Never, 0);
    ordinary_command(&window, &mut daemon, cx, Some(0));
    assert!(
        take_notifications().is_empty(),
        "notify_on_command_finish=never remains a hard gate"
    );

    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);
    ordinary_command(&window, &mut daemon, cx, Some(0));
    let notes = take_notifications();
    assert_eq!(
        notes.len(),
        1,
        "the ordinary path delivered once: {notes:?}"
    );
    assert!(
        notes[0].body.contains("succeeded"),
        "zero is reported as success: {:?}",
        notes[0].body
    );
    assert_eq!(
        notes[0].title, "lyrel",
        "the local pane keeps its app title"
    );
    assert_eq!(
        notes[0].pane,
        Some(window.update(cx, |_, _, cx| cx.entity_id()).unwrap()),
        "the retained OS routing still reveals this pane"
    );
}

#[gpui_kit::test]
fn rich_agent_hooks_suppress_ordinary_command_completion(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    use lyrel_core::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let _ = take_notifications();
    let (window, mut daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);

    lyrel_core::daemon::protocol::DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
        .encode(&mut daemon)
        .unwrap();
    lyrel_core::daemon::protocol::DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        status: AgentStatus::Working,
        session_id: Some("sid-command-notification".into()),
        launch_argv: Some(vec!["omp".into()]),
        rich: true,
        ..Default::default()
    })))
    .encode(&mut daemon)
    .unwrap();
    wait_for_shell(
        &window,
        cx,
        |view| view.agent_session().is_some_and(|session| session.rich),
        "the rich agent session",
    );
    report_prompt(&mut daemon, true, None);
    wait_for_shell(&window, cx, |view| view.terminal.at_prompt(), "a prompt");
    pump_foreground(cx);

    ordinary_command(&window, &mut daemon, cx, Some(0));
    assert!(
        take_notifications().is_empty(),
        "rich hooks own this agent outcome; ordinary completion must not duplicate it"
    );
}

/// A nested shell or `ssh` relay publishes its `D` with `at_prompt` held false
/// by the daemon. That is not a local command boundary even when it carries an
/// exit code, so only the outer command's accepted result may notify.
#[gpui_kit::test]
fn a_relayed_result_does_not_replace_the_outer_commands_outcome(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, mut daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);

    report_prompt(&mut daemon, true, None);
    wait_for_shell(&window, cx, |view| view.terminal.at_prompt(), "a prompt");
    report_prompt(&mut daemon, false, None);
    wait_for_shell(
        &window,
        cx,
        |view| !view.terminal.at_prompt(),
        "the command to start",
    );
    pump_foreground(cx);

    report_prompt(&mut daemon, false, Some(23));
    wait_for_shell(
        &window,
        cx,
        |view| view.terminal.last_exit_code() == Some(23),
        "the relayed result",
    );
    pump_foreground(cx);
    assert!(
        take_notifications().is_empty(),
        "a relayed result never becomes a local completion"
    );

    // The outer shell drew only A/B: without its own D, its result is unknown.
    report_prompt(&mut daemon, true, None);
    wait_for_shell(
        &window,
        cx,
        |view| view.terminal.at_prompt() && view.terminal.last_exit_code().is_none(),
        "the outer prompt without a local result",
    );
    pump_foreground(cx);
    let notes = take_notifications();
    assert_eq!(notes.len(), 1, "only the outer command notified: {notes:?}");
    assert!(
        notes[0].body.contains("exit status unknown"),
        "an absent outer status is explicitly unknown: {:?}",
        notes[0].body
    );
    assert!(
        !notes[0].body.contains("23"),
        "the relayed nested status is not attributed to the outer command: {:?}",
        notes[0].body
    );

    ordinary_command(&window, &mut daemon, cx, Some(7));
    let notes = take_notifications();
    assert_eq!(notes.len(), 1, "the next local command notified: {notes:?}");
    assert!(
        notes[0].body.contains("exit code 7"),
        "a genuine local D;7 remains authoritative: {:?}",
        notes[0].body
    );
}

#[gpui_kit::test]
fn title_events_drive_the_tab_title(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.title, "lyrel");
            view.handle_event(PaneEvent::Title("vim — main.rs".into()), cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.title, "vim — main.rs");
            view.handle_event(PaneEvent::ResetTitle, cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| assert_eq!(view.title, "lyrel"))
        .unwrap();
}

#[gpui_kit::test]
fn native_history_title_seeds_only_the_placeholder_or_stale_shell_title(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.seed_session_title("Recovered session", cx);
            assert_eq!(view.title, "Recovered session");
            view.title = r"C:\Windows\System32\cmd.exe".into();
            view.seed_session_title("Named session", cx);
            assert_eq!(view.title, "Named session");
            view.title = "Latest native title".into();
            view.seed_session_title("Stale history title", cx);
            assert_eq!(view.title, "Latest native title");
        })
        .unwrap();
}

/// The flash this wait exists to stop: a prompt framework names the command
/// it is about to run and puts the directory back at the next prompt, and
/// for anything that finishes in a blink both edges arrive within a few
/// frames. The tab used to show the command and snap back, which reads as a
/// glitch rather than as information.
#[gpui_kit::test]
fn a_command_over_before_the_wait_never_reaches_the_tab(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.default_title = "~/dev".into();
            view.title = "~/dev".into();
            view.handle_event(PaneEvent::Title("ls".into()), cx);
            view.handle_event(PaneEvent::Title("~/dev".into()), cx);
            assert_eq!(view.title, "~/dev", "and never flashed on the way");
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.title, "~/dev",
                "the command was over before the tab could adopt its name"
            );
        })
        .unwrap();
}

/// A program that rewrites its own title faster than the wait — a download
/// reporting progress — still updates the tab. The wait already running
/// adopts the newest title rather than starting over, which is what would
/// leave the tab frozen on the directory for as long as the download ran.
#[gpui_kit::test]
fn a_title_rewritten_faster_than_the_wait_still_lands(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.handle_event(PaneEvent::Title("wget 1%".into()), cx);
            view.handle_event(PaneEvent::Title("wget 40%".into()), cx);
            view.handle_event(PaneEvent::Title("wget 99%".into()), cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| assert_eq!(view.title, "wget 99%"))
        .unwrap();
}

/// An SSH pane answers to the host it dialled, and keeps answering to it
/// after the remote program hands the title back (#438). Before this every
/// SSH tab in the window read "lyrel" until — and only if — the far shell
/// had integration enough to title itself.
#[gpui_kit::test]
fn an_ssh_pane_is_named_after_its_host(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.default_title = "prod-web".into();
            view.title = "prod-web".into();
            view.handle_event(PaneEvent::Title("vim — main.rs".into()), cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.title, "vim — main.rs");
            view.handle_event(PaneEvent::ResetTitle, cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.title, "prod-web",
                "a reset goes back to the host, not to the app's own name"
            );
        })
        .unwrap();
}

#[test]
fn shim_detection_names_known_wrappers_only() {
    assert_eq!(known_pty_shim("zsh (kiro-cli-term)"), Some("kiro-cli-term"));
    assert_eq!(known_pty_shim("figterm"), Some("figterm"));
    assert_eq!(known_pty_shim("qterm"), Some("qterm"));
    assert_eq!(known_pty_shim("ssh"), None);
    assert_eq!(known_pty_shim("wezterm"), None);
    assert_eq!(known_pty_shim(""), None);
    assert!(integration_notice_message(Some("kiro-cli-term")).contains("kiro-cli-term"));
    assert!(!integration_notice_message(None).contains("intercepting"));
}

#[gpui_kit::test]
fn ctrl_r_without_integration_raises_the_notice_once(cx: &mut TestAppContext) {
    lyrel_ui_base::config::pin_test_config_dir();

    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            let ctrl_r = KeyDownEvent {
                keystroke: key("ctrl-r"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.integration_notice.is_none(),
                "the grace window stays silent"
            );

            view.created_at = std::time::Instant::now() - INTEGRATION_GRACE * 2;
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.integration_notice.is_some(),
                "Ctrl+R raises the notice"
            );
            cx.notify();
        })
        .unwrap();

    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(
                view.integration_notice.is_some(),
                "the notice survives a real render pass"
            );

            let ctrl_r = KeyDownEvent {
                keystroke: key("ctrl-r"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.integration_notice.is_none(),
                "a keystroke dismisses the notice"
            );
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.integration_notice.is_none(),
                "the notice is one-shot per pane"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn a_dropped_link_does_not_claim_the_process_exited(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            bind_to_a_disconnected_remote_workspace(view, cx);
            view.handle_event(PaneEvent::Exit, cx);
            assert_eq!(view.title, "lyrel — disconnected");

            view.set_workspace(None);
            view.handle_event(PaneEvent::Exit, cx);
            assert_eq!(view.title, "lyrel — process exited");
        })
        .unwrap();
}

/// A workspace pane answers to its workspace's name: untitled tabs show
/// it, and a dead link's suffix hangs off it — not off the bare app name,
/// which read "lyrel — disconnected" no matter whose link died.
#[gpui_kit::test]
fn a_workspace_pane_answers_to_its_workspaces_name(cx: &mut TestAppContext) {
    use lyrel_ui_base::route::PaneWorkspace;
    use lyrel_ui_base::session::{RemoteTarget, WorkspaceId};
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.set_workspace(Some(PaneWorkspace {
                workspace: WorkspaceId::new(),
                target: RemoteTarget::direct("me", "build-box", 22),
                spec: None,
                label: Some("hummingbot".into()),
                resize_echo: false,
            }));
            assert_eq!(view.title, "hummingbot", "an untitled tab shows the name");
            view.handle_event(PaneEvent::Exit, cx);
            assert_eq!(view.title, "hummingbot — disconnected");
        })
        .unwrap();
}

/// Holding Backspace on an empty bash prompt (or Tab with nothing to
/// complete) rings the bell at key-repeat rate. Every flash used to arm its
/// own clear timer, so the first bell's timer blanked a flash the fifth bell
/// had just re-lit, and the pane strobed for as long as the key was held
/// (#874).
#[gpui_kit::test]
fn a_bell_rung_at_key_repeat_rate_holds_one_steady_flash(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    let lit = |cx: &mut TestAppContext| window.update(cx, |view, _, _| view.bell_flash).unwrap();

    let repeat = std::time::Duration::from_millis(33);
    let mut dark = Vec::new();
    for i in 0..30 {
        window
            .update(cx, |view, _, cx| view.handle_event(PaneEvent::Bell, cx))
            .unwrap();
        cx.executor().advance_clock(repeat);
        cx.run_until_parked();
        if !lit(cx) {
            dark.push(i);
        }
    }
    assert!(
        dark.is_empty(),
        "the flash went dark between bells after repeats {dark:?}"
    );

    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert!(!lit(cx), "the flash outlived the last bell");
}

/// History assertions are scoped per test app: the inbox installs lazily, so
/// read through `ensure`.
fn record_count(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| {
        lyrel_ui_base::notification_history::ensure(cx);
        lyrel_ui_base::notification_history::history(cx)
            .read(cx)
            .records()
            .count()
    })
}

fn records_since(cx: &mut TestAppContext, baseline: usize) -> Vec<(Severity, String)> {
    cx.update(|cx| {
        lyrel_ui_base::notification_history::ensure(cx);
        lyrel_ui_base::notification_history::history(cx)
            .read(cx)
            .records()
            .skip(baseline)
            .map(|record| (record.severity(), record.message().to_string()))
            .collect()
    })
}

/// Name the pane so a command result's record is attributable to this test
/// (the running title becomes the command label).
fn name_pane(window: &gpui_kit::WindowHandle<TerminalView>, cx: &mut TestAppContext, name: &str) {
    window
        .update(cx, |view, _, _| {
            view.default_title = name.to_string();
            view.title = name.to_string();
        })
        .unwrap();
}

/// Failed and unknown exit results are eligible events: they enter the
/// history with their own severities even while the desktop timing mode
/// holds every banner back.
#[gpui_kit::test]
fn failed_and_unknown_command_results_record_with_desktop_delivery_closed(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, mut daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Never, 0);
    let baseline = record_count(cx);

    name_pane(&window, cx, "t395-failed");
    ordinary_command(&window, &mut daemon, cx, Some(7));
    assert!(
        take_notifications().is_empty(),
        "the Never mode keeps its hard gate on desktop delivery"
    );
    let recorded = records_since(cx, baseline);
    assert_eq!(
        recorded.len(),
        1,
        "one eligible command result records once: {recorded:?}"
    );
    assert_eq!(
        recorded[0].0,
        Severity::Error,
        "an exit code is authoritative"
    );
    assert!(
        recorded[0].1.contains("t395-failed") && recorded[0].1.contains("exit code 7"),
        "the record names the command and its failure: {:?}",
        recorded[0].1
    );

    name_pane(&window, cx, "t395-unknown");
    ordinary_command(&window, &mut daemon, cx, None);
    let recorded = records_since(cx, baseline);
    assert_eq!(
        recorded.len(),
        2,
        "the unknown outcome records as its own event: {recorded:?}"
    );
    assert_eq!(
        recorded[1].0,
        Severity::Warning,
        "an absent result is preserved as unknown, not success"
    );
    assert!(
        recorded[1].1.contains("t395-unknown") && recorded[1].1.contains("unknown"),
        "the record names the command and the unknown status: {:?}",
        recorded[1].1
    );
}

/// A command result is one qualified event: two viewers of the same daemon
/// pane land on the same stable identity, so the history holds one record
/// and the desktop shows one banner.
#[gpui_kit::test]
fn one_command_result_records_once_across_two_viewers(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    // Two windows attached to pane 1 the way two viewers share a daemon pane.
    let (window_a, mut daemon_a) = harness(cx);
    let (window_b, mut daemon_b) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);
    let baseline = record_count(cx);

    name_pane(&window_a, cx, "t395-shared");
    name_pane(&window_b, cx, "t395-shared");
    ordinary_command(&window_a, &mut daemon_a, cx, Some(3));
    ordinary_command(&window_b, &mut daemon_b, cx, Some(3));
    assert_eq!(
        take_notifications().len(),
        1,
        "one banner for one event, no matter how many viewers saw it"
    );
    let recorded = records_since(cx, baseline);
    assert_eq!(
        recorded.len(),
        1,
        "one history entry across viewers: {recorded:?}"
    );
    assert!(recorded[0].1.contains("t395-shared"));
}

/// Report `state` on `daemon` and wait for `pane` to hold it.
fn report_agent_state(
    state: lyrel_core::core::cli_agent::AgentSessionState,
    pane: &gpui_kit::Entity<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut lyrel_core::daemon::transport::Stream,
) {
    lyrel_core::daemon::protocol::DaemonMsg::AgentStatus(Some(Box::new(state.clone())))
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

fn agent_state(
    status: lyrel_core::core::cli_agent::AgentStatus,
    turns: u64,
    sid: &str,
    message: Option<&str>,
) -> lyrel_core::core::cli_agent::AgentSessionState {
    lyrel_core::core::cli_agent::AgentSessionState {
        status,
        message: message.map(str::to_string),
        session_id: Some(sid.into()),
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
    }
}

/// Poll `pane`'s agent status the way the foreground lifecycle does.
fn poll_agent(
    window: &gpui_kit::WindowHandle<TerminalView>,
    pane: &gpui_kit::Entity<TerminalView>,
    cx: &mut TestAppContext,
) {
    cx.update_window((*window).into(), |_, window, cx| {
        pane.update(cx, |pane, cx| {
            pane.poll_agent_status(true, window, cx);
        })
    })
    .unwrap();
}

fn run_out_agent_settle(cx: &mut TestAppContext) {
    cx.executor()
        .advance_clock(crate::agent_activity::DONE_SETTLE * 2);
    cx.run_until_parked();
}

/// The completion settle window is a gate on the record too: the Done notice
/// lands in history and on the desktop only after 1.5 s hold, and a status
/// that supersedes it during the wait cancels both.
#[gpui_kit::test]
fn agent_done_records_after_settle_and_superseded_done_leaves_nothing(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, _root_daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_test_pane(39501, window, cx)
        })
        .unwrap();
    let baseline = record_count(cx);

    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Working,
            0,
            "sid-t395-settle",
            None,
        ),
        &pane,
        cx,
        &mut daemon,
    );
    poll_agent(&window, &pane, cx);
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Done,
            1,
            "sid-t395-settle",
            None,
        ),
        &pane,
        cx,
        &mut daemon,
    );
    poll_agent(&window, &pane, cx);
    assert_eq!(
        record_count(cx),
        baseline,
        "a completion still settling is not a record yet"
    );

    run_out_agent_settle(cx);
    let recorded = records_since(cx, baseline);
    assert_eq!(
        recorded.len(),
        1,
        "the settled result recorded exactly once: {recorded:?}"
    );
    assert_eq!(recorded[0].0, Severity::Info);
    let delivered = take_notifications();
    assert_eq!(delivered.len(), 1, "and it interrupted once: {delivered:?}");

    // A queued question resumes work before Done is stable: the pending
    // completion is superseded and neither records nor delivers, while the
    // wait itself is an eligible event of its own.
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Working,
            1,
            "sid-t395-settle",
            None,
        ),
        &pane,
        cx,
        &mut daemon,
    );
    poll_agent(&window, &pane, cx);
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Done,
            2,
            "sid-t395-settle",
            None,
        ),
        &pane,
        cx,
        &mut daemon,
    );
    poll_agent(&window, &pane, cx);
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Waiting,
            2,
            "sid-t395-settle",
            Some("Resume the turn, t395?"),
        ),
        &pane,
        cx,
        &mut daemon,
    );
    poll_agent(&window, &pane, cx);
    run_out_agent_settle(cx);
    let delivered = take_notifications();
    assert_eq!(
        delivered.len(),
        1,
        "the superseding wait interrupted once, the cancelled done never: {delivered:?}"
    );
    let recorded = records_since(cx, baseline);
    assert_eq!(
        recorded.len(),
        2,
        "settled done + the wait that superseded it, nothing else: {recorded:?}"
    );
    assert!(
        recorded[1].1.contains("Resume the turn, t395?"),
        "the wait record carries the question: {:?}",
        recorded[1].1
    );
}

/// A replayed agent status is not a new event: the rebuilt pane's first
/// sight of a settled turn restores the read mark instead of notifying, and
/// the history keeps the single record the live view created.
#[gpui_kit::test]
fn replayed_agent_turn_does_not_recreate_its_history_record(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, _root_daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);
    let (before, mut before_daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_test_pane(39502, window, cx)
        })
        .unwrap();
    let baseline = record_count(cx);

    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Working,
            0,
            "sid-t395-replay",
            None,
        ),
        &before,
        cx,
        &mut before_daemon,
    );
    poll_agent(&window, &before, cx);
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Done,
            1,
            "sid-t395-replay",
            None,
        ),
        &before,
        cx,
        &mut before_daemon,
    );
    poll_agent(&window, &before, cx);
    run_out_agent_settle(cx);
    assert_eq!(
        records_since(cx, baseline).len(),
        1,
        "the live view recorded the settled turn"
    );

    // The same daemon pane, reattached the way a restored tab is: the replay
    // is where this pane starts, not a result that just arrived.
    let (after, mut after_daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(39502, window, cx)
        })
        .unwrap();
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Done,
            1,
            "sid-t395-replay",
            None,
        ),
        &after,
        cx,
        &mut after_daemon,
    );
    poll_agent(&window, &after, cx);
    run_out_agent_settle(cx);
    assert!(
        take_notifications().is_empty(),
        "the replay did not interrupt at all"
    );
    assert_eq!(
        records_since(cx, baseline).len(),
        1,
        "no event recreation on replay"
    );
}

/// Program-written notices enter the history through the same budget that
/// bounds the desktop, and the rate-limited remainder is said once.
#[gpui_kit::test]
fn osc_notes_record_within_the_ingress_budget(cx: &mut TestAppContext) {
    use lyrel_core::core::osc::{NOTE_WINDOW, NOTES_PER_WINDOW};
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, mut daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);
    let baseline = record_count(cx);

    let mut flood = Vec::new();
    for index in 0..NOTES_PER_WINDOW + 2 {
        flood.extend(format!("\x1b]9;t395-flood-{index}\x1b\\").into_bytes());
    }
    lyrel_core::daemon::protocol::DaemonMsg::Output(flood)
        .encode(&mut daemon)
        .unwrap();
    // Pump until the whole batch has been read, then let the budget window
    // roll so the dropped remainder is said once. Deliveries accumulate
    // across pumps — each drain takes only what landed since the last one.
    let mut delivered = Vec::new();
    for _ in 0..400 {
        pump_foreground(cx);
        delivered.extend(take_notifications());
        if delivered.len() >= NOTES_PER_WINDOW {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.executor().advance_clock(NOTE_WINDOW * 2);
    pump_foreground(cx);
    delivered.extend(take_notifications());
    assert_eq!(
        delivered.len(),
        NOTES_PER_WINDOW + 1,
        "the budget admitted its five and one note said the rest: {delivered:?}"
    );
    let recorded = records_since(cx, baseline);
    assert_eq!(
        recorded.len(),
        NOTES_PER_WINDOW + 1,
        "history intake keeps the same limits: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .take(NOTES_PER_WINDOW)
            .all(|(severity, message)| *severity == Severity::Info
                && message.starts_with("t395-flood-")),
        "each admitted note is its own record: {recorded:?}"
    );
    assert!(
        recorded[NOTES_PER_WINDOW].1.contains("weren't shown"),
        "the dropped-notes note is recorded once too: {:?}",
        recorded[NOTES_PER_WINDOW].1
    );
}

/// A pane whose agent reports through lyrel's hooks gets its Waiting and
/// Done notices from the hook path; the program's own OSC copy is their
/// duplicate and must not enter the history either.
#[gpui_kit::test]
fn hooked_agent_program_notes_are_swallowed_before_the_history(cx: &mut TestAppContext) {
    let _capture = crate::desktop_notification::capture_test_notifications();
    let _ = take_notifications();
    let (window, mut daemon) = harness(cx);
    let _ = take_notifications();
    lyrel_ui_base::i18n::set_locale("en");
    configure_notifications(cx, lyrel_core::core::config::NotifyMode::Always, 0);
    let baseline = record_count(cx);

    lyrel_core::daemon::protocol::DaemonMsg::Agent(Some(
        lyrel_core::core::cli_agent::CLIAgent::OhMyPi,
    ))
    .encode(&mut daemon)
    .unwrap();
    report_agent_state(
        agent_state(
            lyrel_core::core::cli_agent::AgentStatus::Working,
            0,
            "sid-t395-hooks",
            None,
        ),
        &window.update(cx, |_, _, cx| cx.entity()).unwrap(),
        cx,
        &mut daemon,
    );
    lyrel_core::daemon::protocol::DaemonMsg::Output(b"\x1b]9;t395-hook-duplicate\x1b\\".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..400 {
        pump_foreground(cx);
        if !take_notifications().is_empty() || record_count(cx) > baseline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.executor()
        .advance_clock(lyrel_core::core::osc::NOTE_WINDOW * 2);
    pump_foreground(cx);
    take_notifications();
    assert_eq!(
        record_count(cx),
        baseline,
        "the hook duplicate never lands in the history"
    );
}
