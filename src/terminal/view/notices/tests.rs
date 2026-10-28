use super::*;
use crate::terminal::view::test_support::*;
use gpui_kit::TestAppContext;

/// What the label ladder asks a pane: are you showing a name of your own,
/// or still standing under the app's? (#740)
#[test]
fn a_pane_states_a_title_whenever_it_is_not_the_placeholder() {
    use crate::terminal::view::stated_title;

    // Nothing has spoken — this is the pane a directory stands in for.
    assert_eq!(stated_title("lyrel"), None);
    assert_eq!(stated_title("  lyrel  "), None);
    assert_eq!(stated_title("   "), None);

    // A title from the program running in it.
    assert_eq!(stated_title("vim — main.rs"), Some("vim — main.rs"));
    assert_eq!(stated_title(" user@host:~/repo "), Some("user@host:~/repo"));
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
    crate::core::config::pin_test_config_dir();

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
    use crate::core::session::{RemoteTarget, WorkspaceId};
    use crate::terminal::PaneWorkspace;
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
    let lit =
        |cx: &mut TestAppContext| window.update(cx, |view, _, _| view.bell_flash).unwrap();

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

