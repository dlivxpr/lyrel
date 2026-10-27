use super::*;

/// Coming back from an `ssh` session left the pane with no history at all:
/// the scope switch cleared the list, and the reload that refills it is a
/// background task, so ↑ recalled nothing until that landed (#817).
#[gpui_kit::test]
fn a_pane_back_from_ssh_still_recalls_its_local_history(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            seed_history(view, vec!["cargo build".to_string(), "ssh box".to_string()]);
        })
        .unwrap();

    let away = crate::daemon::protocol::RemoteContext {
        kind: crate::daemon::protocol::RemoteKind::Ssh,
        argv: vec!["ssh".into(), "box".into()],
        target: "box".into(),
    };
    DaemonMsg::RemoteContext(Some(away))
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.remote_context().is_some())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, _, cx| view.follow_history_scope(cx))
        .unwrap();

    DaemonMsg::RemoteContext(None).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.remote_context().is_none())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            view.follow_history_scope(cx);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.input.editor().text(),
                "ssh box",
                "↑ right after the ssh session ended recalled nothing"
            );
        })
        .unwrap();
}

/// The cache above must not hand a scope someone else's list: stepping into
/// an `ssh` session still starts from nothing until the far end's own
/// history is read.
#[gpui_kit::test]
fn a_pane_going_out_to_ssh_does_not_inherit_the_local_history(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            seed_history(view, vec!["rm -rf ./build".to_string()]);
        })
        .unwrap();

    DaemonMsg::RemoteContext(Some(crate::daemon::protocol::RemoteContext {
        kind: crate::daemon::protocol::RemoteKind::Ssh,
        argv: vec!["ssh".into(), "box".into()],
        target: "box".into(),
    }))
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.remote_context().is_some())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            view.follow_history_scope(cx);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.input.editor().text(),
                "",
                "a local command was recalled onto a remote prompt"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn submitted_command_backfills_its_exit_code(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = crate::core::config::config_dir_path().expect("a config dir resolves");

    let (window, mut daemon) = harness(cx);
    let wait = |cx: &mut TestAppContext, pred: &dyn Fn(&TerminalView) -> bool, what: &str| {
        for _ in 0..200 {
            if window.update(cx, |view, _, _| pred(view)).unwrap() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    };

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait(cx, &|v| v.terminal.at_prompt(), "the initial prompt report");

    let marker = format!("lyrel_gpui_exit_marker_{}", std::process::id());
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set(&marker);
            view.submit_command(cx);
            assert!(view.history.has_pending(), "record defers for the exit");
        })
        .unwrap();

    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(3),
    }
    .encode(&mut daemon)
    .unwrap();
    wait(
        cx,
        &|v| v.terminal.at_prompt() && v.terminal.last_exit_code() == Some(3),
        "the post-command prompt report",
    );

    window
        .update(cx, |view, window, cx| {
            view.poll_foreground(window, cx);
            assert!(!view.history.has_pending(), "poll flushed the record");
            assert_eq!(
                view.history.metadata(&marker).and_then(|m| m.exit),
                Some(3),
                "in-memory metadata learned the exit code"
            );
        })
        .unwrap();

    let content = std::fs::read_to_string(dir.join("history")).expect("history file written");
    let line = content
        .lines()
        .find(|l| l.contains(&marker))
        .expect("the submitted command was recorded");
    let mut fields = line.splitn(4, '\t');
    let ts = fields.next().unwrap();
    assert!(!ts.is_empty() && ts.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(fields.next(), Some("3"), "exit code field");
}

#[gpui_kit::test]
fn a_late_history_load_cannot_replace_a_restored_scope(cx: &mut TestAppContext) {
    use crate::terminal::history::Scope;
    let (window, _daemon) = harness(cx);
    window.update(cx, |view, _, cx| {
        seed_history(view, vec!["echo local".into()]);
        let prompt = view.history_prompt();
        let remote = view.history.change_scope(Scope::remote("late-history-box"), view.host_id, prompt).unwrap();
        view.history.change_scope(Scope::Local, view.host_id, prompt).unwrap();
        assert!(!view.history.accept_load(remote, view.host_id, history_fixture(vec!["echo wrong-host".into()])));
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "echo local");
    }).unwrap();
}

#[gpui_kit::test]
fn a_scope_round_trip_rejects_the_previous_load_and_never_caches_a_placeholder(cx: &mut TestAppContext) {
    use crate::terminal::history::Scope;
    let (window, _daemon) = harness(cx);
    window.update(cx, |view, _, cx| {
        seed_history(view, vec!["echo local".into()]);
        let prompt = view.history_prompt();
        let scope = Scope::remote("round-trip-history-box");
        let retired = view.history.change_scope(scope.clone(), view.host_id, prompt).unwrap();
        view.history.change_scope(Scope::Local, view.host_id, prompt).unwrap();
        let current = view.history.change_scope(scope, view.host_id, prompt).unwrap();
        assert!(!view.history.accept_load(retired, view.host_id, history_fixture(vec!["echo retired".into()])));
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "", "the incomplete remote list did not inherit local commands");
        assert!(view.history.accept_load(current, view.host_id, history_fixture(vec!["echo current".into()])));
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "echo current");
        view.history.change_scope(Scope::Local, view.host_id, prompt).unwrap();
        view.history.change_scope(Scope::remote("round-trip-history-box"), view.host_id, prompt).unwrap();
        view.input.editor_mut().clear();
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "echo current", "a complete scope is recalled immediately");
    }).unwrap();
}

#[gpui_kit::test]
fn a_history_load_is_fenced_when_the_host_changes_before_the_next_poll(cx: &mut TestAppContext) {
    use crate::terminal::history::Scope;
    use crate::ui::host_ops::HostId;
    let (window, _daemon) = harness(cx);
    window.update(cx, |view, _, cx| {
        let prompt = view.history_prompt();
        let scope = Scope::remote("shared-label-history-box");
        let retired = view.history.change_scope(scope.clone(), view.host_id, prompt).unwrap();
        view.host_id = HostId::from_connection_key("ssh-direct:other-history-box:22");
        assert!(!view.history.accept_load(retired, view.host_id, history_fixture(vec!["echo old-host".into()])));
        let current = view.history.change_scope(scope, view.host_id, prompt).unwrap();
        assert!(view.history.accept_load(current, view.host_id, history_fixture(vec!["echo new-host".into()])));
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "echo new-host");
    }).unwrap();
}

#[gpui_kit::test]
fn equally_named_history_scopes_never_restore_another_hosts_commands(cx: &mut TestAppContext) {
    use crate::terminal::history::Scope;
    use crate::ui::host_ops::HostId;
    let (window, _daemon) = harness(cx);
    window.update(cx, |view, _, cx| {
        let prompt = view.history_prompt();
        let scope = Scope::remote("equal-label-history-box");
        let first_host = view.host_id;
        let first = view.history.change_scope(scope.clone(), first_host, prompt).unwrap();
        assert!(view.history.accept_load(first, first_host, history_fixture(vec!["echo first-host".into()])));
        view.host_id = HostId::from_connection_key("ssh-direct:second-history-box:22");
        let second = view.history.change_scope(scope.clone(), view.host_id, prompt).unwrap();
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "", "scope labels alone cannot authorize a cached Host");
        assert!(view.history.accept_load(second, view.host_id, history_fixture(vec!["echo second-host".into()])));
        view.host_id = first_host;
        view.history.change_scope(scope, first_host, prompt).unwrap();
        view.handle_editor_key(&key("up"), cx);
        assert_eq!(view.input.editor().text(), "echo first-host", "the original Host still has its own cached list");
    }).unwrap();
}
