use super::*;
use crate::ui::app::{LyrelApp, test_window};
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::tab::Tab;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

fn pending(
    app: &mut LyrelApp,
    window: &mut Window,
    cx: &mut Context<LyrelApp>,
) -> Entity<PendingPane> {
    let waiting = cx.new(|cx| {
        PendingPane::new(
            "local",
            PendingSpawn {
                workspace: None,
                working_directory: Some(std::path::PathBuf::from("/original")),
                restore_pane: None,
                shell: None,
                agent: None,
                agent_session_id: None,
                agent_launch_argv: None,
                agent_session_scope: None,
                run_on_land: None,
                owner: Some(app.workspace),
                font_size: 14.,
            },
            cx,
        )
    });
    // The same workbench consumer as production; only the daemon reply timing
    // is controlled by this fixture. The terminal still owns a real socket.
    cx.subscribe_in(
        &waiting,
        window,
        |app, pending, _: &SpawnFinished, window, cx| {
            app.land_pane(&pending, window, cx);
        },
    )
    .detach();
    app.tabs
        .push(Tab::new(Pane::leaf(PaneSlot::Connecting(waiting.clone()))));
    app.tabs.select(app.tabs.len() - 1);
    app.focus_active(window, cx);
    cx.notify();
    waiting
}

fn deliver(
    waiting: &Entity<PendingPane>,
    result: Result<ShellParts, String>,
    vcx: &mut VisualTestContext,
) {
    vcx.update(|window, cx| {
        let executor = cx.background_executor().clone();
        waiting.update(cx, |pending, cx| {
            let generation = pending.creation.generation;
            cx.spawn_in(window, async move |this, cx| {
                smol::future::yield_now().await;
                let _ = this.update_in(cx, |pending, window, cx| {
                    pending.receive(
                        generation,
                        result.map(|parts| OwnedTerminal {
                            parts: Some(CreatedTerminal::Shell(parts)),
                            executor,
                        }),
                        window,
                        cx,
                    );
                });
            })
            .detach();
        });
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn a_late_spawn_lands_in_its_own_slot_and_workspace(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original, owner, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        let waiting = pending(app, window, cx);
        app.tabs.select(0);
        app.focus_active(window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(91, Some(app.workspace));
        (waiting, original, (app.workspace, parts), daemon)
    });
    deliver(&waiting, Ok(owner.1), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), original);
        assert_eq!(
            app.tabs.active_index(),
            0,
            "landing never selects the delayed tab"
        );
        let landed = app.tabs[1].pane.terminals();
        assert_eq!(landed.len(), 1);
        assert_eq!(landed[0].read(cx).pane_id, 91);
        assert_eq!(landed[0].read(cx).owner_workspace(), Some(owner.0));
    });
    vcx.update(|window, cx| {
        assert!(
            app.read(cx).tabs[0]
                .pane
                .first_leaf()
                .unwrap()
                .contains_focused(window, cx)
        );
    });
}

#[gpui_kit::test]
fn a_failed_spawn_keeps_its_failure_and_never_overwrites_the_original(cx: &mut TestAppContext) {
    let (app, mut vcx, mut original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        let waiting = pending(app, window, cx);
        app.run_in_active_terminal("do-not-send-to-original", window, cx);
        (waiting, original)
    });
    deliver(&waiting, Err("configured shell missing".into()), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert!(waiting.read(cx).is_failed());
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(
            app.tabs[1].pane.first_leaf().unwrap().entity_id(),
            waiting.entity_id()
        );
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), original);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find("pending-pane-retry").visible(),
            "failure has a recovery control"
        );
        waiting.update(cx, |pending, _| {
            assert!(pending.next_auto_retry().is_none());
            assert_eq!(
                pending.spawn.run_on_land.as_deref(),
                Some("do-not-send-to-original")
            );
            assert_eq!(
                pending.spawn.working_directory.as_deref(),
                Some(std::path::Path::new("/original"))
            );
        });
    });
    original_stream
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .unwrap();
    while let Ok(message) = crate::daemon::protocol::ClientMsg::read(&mut original_stream) {
        assert!(
            !matches!(message, crate::daemon::protocol::ClientMsg::Input(_)),
            "failed new-pane command reached the original pane"
        );
    }
}

#[gpui_kit::test]
fn a_closed_or_replaced_slot_cannot_claim_a_late_success(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        let waiting = pending(app, window, cx);
        app.tabs.remove(1);
        app.tabs.select(0);
        app.focus_active(window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(92, Some(app.workspace));
        (waiting, original, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), original);
        assert_eq!(app.tabs[0].pane.terminals()[0].read(cx).pane_id, 1);
    });
}

#[gpui_kit::test]
fn a_workspace_change_rejects_the_original_slots_late_success(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let waiting = pending(app, window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(93, Some(app.workspace));
        app.workspace = WorkspaceId::new();
        (waiting, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.tabs[1].pane.first_leaf().unwrap().entity_id(),
            waiting.entity_id()
        );
        assert!(app.tabs[1].pane.terminals().is_empty());
        assert_eq!(app.tabs[0].pane.terminals()[0].read(cx).pane_id, 1);
    });
}

#[gpui_kit::test]
fn a_replacement_terminal_is_not_overwritten_by_the_previous_requests_result(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, replacement, parts, _replacement_daemon, _late_daemon) =
        app.update_in(&mut vcx, |app, window, cx| {
            let waiting = pending(app, window, cx);
            let (replacement, replacement_daemon) =
                crate::terminal::view::quiet_test_pane(94, window, cx);
            app.tabs
                .replace_leaf(waiting.entity_id(), PaneSlot::Ready(replacement.clone()));
            let (parts, daemon) =
                crate::terminal::view::quiet_test_shell_parts(95, Some(app.workspace));
            (waiting, replacement, parts, replacement_daemon, daemon)
        });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs.len(), 2);
        let landed = app.tabs[1].pane.terminals();
        assert_eq!(landed.len(), 1);
        assert_eq!(landed[0].entity_id(), replacement.entity_id());
        assert_eq!(landed[0].read(cx).pane_id, 94);
    });
}

#[gpui_kit::test]
fn a_focused_split_hands_focus_to_its_landed_terminal(cx: &mut TestAppContext) {
    use gpui_kit::Focusable as _;

    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let waiting = pending(app, window, cx);
        app.tabs.remove(1);
        app.tabs.select(0);
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        assert!(app.tabs.get_mut(0).unwrap().split_pane(
            original,
            gpui_kit::Axis::Horizontal,
            false,
            PaneSlot::Connecting(waiting.clone()),
        ));
        window.focus(&waiting.read(cx).focus_handle(cx), cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(96, Some(app.workspace));
        (waiting, original, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 1);
        let leaves = app.tabs[0].pane.terminals();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.iter().any(|leaf| leaf.entity_id() == original));
        let landed = leaves
            .iter()
            .find(|leaf| leaf.read(cx).pane_id == 96)
            .unwrap();
        assert!(landed.read(cx).focus_handle.contains_focused(window, cx));
        assert_eq!(
            app.tabs[0].focus_target(cx).unwrap().entity_id(),
            landed.entity_id(),
            "the terminal focus observer remembers the installed entity, not the retired slot"
        );
    });
}

#[gpui_kit::test]
fn a_late_spawn_keeps_the_open_searchs_focus(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let waiting = pending(app, window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(97, Some(app.workspace));
        (waiting, parts, daemon)
    });
    vcx.dispatch_action(crate::core::actions::TogglePalette);
    vcx.run_until_parked();
    let search_focus = vcx.update(|window, cx| {
        assert!(app.read(cx).search.is_open());
        window.focused(cx).expect("search owns window focus")
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    vcx.update(|window, cx| {
        let app = app.read(cx);
        assert!(app.search.is_open());
        assert_eq!(app.tabs[1].pane.terminals()[0].read(cx).pane_id, 97);
        assert_eq!(window.focused(cx), Some(search_focus));
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let app = app.read(cx);
        assert!(!app.search.is_open());
        let landed = app.tabs[1].pane.terminals()[0].read(cx);
        assert!(landed.focus_handle.contains_focused(window, cx));
    });
}

#[test]
#[ignore = "native child for the pending launch environment contract"]
fn pending_scope_child() {
    let Some(path) = std::env::var_os("LYREL_PENDING_SCOPE_OUTPUT") else {
        return;
    };
    let environment: std::collections::BTreeMap<_, _> = [
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "LC_TERMINAL",
        "KITTY_WINDOW_ID",
        "PI_IMAGE_PROTOCOL",
    ]
    .into_iter()
    .map(|key| (key, std::env::var(key).ok()))
    .collect();
    std::fs::write(path, serde_json::to_vec(&environment).unwrap()).unwrap();
}

#[gpui_kit::test]
fn a_connecting_mapped_launch_keeps_its_agent_environment(cx: &mut TestAppContext) {
    connecting_agent_environment(cx, true);
}

#[gpui_kit::test]
fn a_connecting_known_launch_keeps_opaque_wrapper_intent(cx: &mut TestAppContext) {
    connecting_agent_environment(cx, false);
}

fn connecting_agent_environment(cx: &mut TestAppContext, mapped: bool) {
    use crate::core::shell_quote::quote_for_shell;
    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join(if cfg!(windows) {
        "mapped-hermes.exe"
    } else {
        "mapped-hermes"
    });
    std::fs::copy(std::env::current_exe().unwrap(), &wrapper).unwrap();
    let shell = if cfg!(windows) { "pwsh" } else { "/bin/sh" };
    let command = format!(
        "PI_IMAGE_PROTOCOL=none {} --ignored --exact ui::pending_pane::creation::consumer_tests::pending_scope_child --nocapture",
        quote_for_shell(&wrapper.to_string_lossy(), Some(shell))
    );
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, parts, mut daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let cfg = cx.global_mut::<crate::core::config::Config>();
        cfg.shell = Some(crate::core::config::ShellConfig {
            program: shell.into(),
            args: Vec::new(),
        });
        if mapped {
            cfg.agent_commands
                .insert("mapped-hermes".into(), "hermes".into());
        }
        let waiting = pending(app, window, cx);
        let slot = PaneSlot::Connecting(waiting.clone());
        if mapped {
            crate::ui::agent_launch::run_when_ready(&slot, command, cx);
        } else {
            crate::ui::agent_launch::run_known_agent_when_ready(
                &slot,
                command,
                Some(crate::core::cli_agent::CLIAgent::Hermes),
                cx,
            );
        }
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(97, Some(app.workspace));
        (waiting, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(20)))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let line = loop {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        vcx.run_until_parked();
        if let Ok(crate::daemon::protocol::ClientMsg::Input(bytes)) =
            crate::daemon::protocol::ClientMsg::read(&mut daemon)
        {
            break String::from_utf8(bytes)
                .unwrap()
                .trim_end_matches('\r')
                .to_owned();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pending command never reached its landed pane"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let output = dir.path().join("environment.json");
    let mut process = std::process::Command::new(shell);
    process.args(if cfg!(windows) {
        vec!["-NoProfile", "-Command"]
    } else {
        vec!["-c"]
    });
    process
        .env("LYREL_PENDING_SCOPE_OUTPUT", &output)
        .env("TERM_PROGRAM", "lyrel");
    if mapped {
        process.env_remove("KITTY_WINDOW_ID");
    } else {
        process.env("KITTY_WINDOW_ID", "owned-window");
    }
    let result = process.arg(line).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let actual: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(actual["TERM_PROGRAM"], "kitty");
    assert_eq!(actual["TERM_PROGRAM_VERSION"], "");
    assert_eq!(actual["LC_TERMINAL"], "lyrel");
    assert_eq!(
        actual["KITTY_WINDOW_ID"],
        if mapped { "lyrel" } else { "owned-window" }
    );
    assert_eq!(actual["PI_IMAGE_PROTOCOL"], "none");
}

/// A resume/fork command is already fully scoped by the session operation.
/// Typing it must not run the launch encoder a second time: the Hermes
/// presence marker is a shell expansion, and the literal argv parser rejects
/// `$` on purpose. The regression is the public pane seam: the exact prepared
/// line reaches the shell and the child observes the marker semantics.
#[gpui_kit::test]
fn a_prepared_hermes_command_is_typed_verbatim_and_keeps_its_marker(cx: &mut TestAppContext) {
    use crate::core::shell_quote::quote_for_shell;
    let dir = tempfile::tempdir().unwrap();
    // The defect is POSIX-specific: only there does the prepared marker use
    // `${KITTY_WINDOW_ID-lyrel}`. Prefer an available POSIX shell on Windows
    // too, and fall back to pwsh only when none is installed.
    let shell = if cfg!(windows) {
        let posix = std::process::Command::new("sh")
            .args(["-c", "true"])
            .status()
            .is_ok_and(|status| status.success());
        if posix { "sh" } else { "pwsh" }
    } else {
        "/bin/sh"
    };
    let posix = shell == "sh" || shell == "/bin/sh";
    let wrapper = dir.path().join(if cfg!(windows) {
        "hermes.exe"
    } else {
        "hermes"
    });
    std::fs::copy(std::env::current_exe().unwrap(), &wrapper).unwrap();
    let native = format!(
        "{} --ignored --exact ui::pending_pane::creation::consumer_tests::pending_scope_child --nocapture",
        quote_for_shell(&wrapper.to_string_lossy(), Some(shell))
    );
    let command = lyrel_core::host::session_operations::scoped_agent_launch_command(
        &*lyrel_core::host::local::LocalHost::new(),
        crate::core::cli_agent::CLIAgent::Hermes,
        &native,
        Some(shell),
    )
    .unwrap();
    assert!(
        command.contains("KITTY_WINDOW_ID"),
        "the prepared command carries the presence-sensitive marker: {command}"
    );

    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, parts, mut daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let cfg = cx.global_mut::<crate::core::config::Config>();
        cfg.shell = Some(crate::core::config::ShellConfig {
            program: shell.into(),
            args: Vec::new(),
        });
        let waiting = pending(app, window, cx);
        let slot = PaneSlot::Connecting(waiting.clone());
        crate::ui::agent_launch::run_prepared_when_ready(&slot, command.clone(), cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(97, Some(app.workspace));
        (waiting, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(20)))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let line = loop {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        vcx.run_until_parked();
        if let Ok(crate::daemon::protocol::ClientMsg::Input(bytes)) =
            crate::daemon::protocol::ClientMsg::read(&mut daemon)
        {
            break String::from_utf8(bytes)
                .unwrap()
                .trim_end_matches('\r')
                .to_owned();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the prepared command never reached its landed pane"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let normalize = |text: &str| text.replace("\r\n", "\n").replace('\r', "\n");
    assert_eq!(
        normalize(&line),
        normalize(&command),
        "a prepared command is already scoped and must be typed verbatim"
    );

    for (inherited, expected) in [(None, "lyrel"), (Some("owned-window"), "owned-window")] {
        let output = dir.path().join(format!("marker-{expected}.json"));
        let mut process = std::process::Command::new(shell);
        process.args(if posix {
            vec!["-c"]
        } else {
            vec!["-NoProfile", "-Command"]
        });
        process
            .env("LYREL_PENDING_SCOPE_OUTPUT", &output)
            .env("TERM_PROGRAM", "lyrel");
        if let Some(value) = inherited {
            process.env("KITTY_WINDOW_ID", value);
        } else {
            process.env_remove("KITTY_WINDOW_ID");
        }
        let result = process.arg(&line).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let actual: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(actual["TERM_PROGRAM"], "kitty");
        assert_eq!(actual["TERM_PROGRAM_VERSION"], "");
        assert_eq!(actual["LC_TERMINAL"], "lyrel");
        assert_eq!(
            actual["KITTY_WINDOW_ID"], expected,
            "the marker defaults only when the target shell has no value"
        );
    }
}
