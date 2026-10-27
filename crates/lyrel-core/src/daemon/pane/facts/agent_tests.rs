use super::super::test_support::{drain, null_writer, test_state, ws};
use super::super::*;
use super::output_marks::OscSniffer;
use super::*;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn live_pty_child_argv_detects_the_agent() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new("bash");
    cmd.args(["-c", "exec -a pi cat"]);
    let mut child = pty.slave.spawn_command(cmd).expect("spawn child");
    let master = Mutex::new(Some(pty.master));

    let mut detected = None;
    for _ in 0..200 {
        if let Some(agent) = foreground_agent(&master).flatten() {
            detected = Some(agent);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();

    let (agent, argv) = detected.expect("agent detected from live PTY child");
    assert_eq!(
        agent,
        crate::core::cli_agent::CLIAgent::Pi,
        "a live PTY child with argv[0]=pi must be detected as Pi"
    );
    assert_eq!(
        argv.first().map(String::as_str),
        Some("pi"),
        "the observed argv rides along with the detection"
    );
}

#[cfg(windows)]
#[test]
fn resumed_output_marks_do_not_release_the_running_agent() {
    use crate::core::cli_agent::CLIAgent;

    let mut sniffer = OscSniffer::new();
    let mut state = test_state(true);
    (&mut state).with_facts(|facts, emit| {
        apply_signals(facts, sniffer.feed(b"\x1b]133;C;omp;aid=lyrel\x07"), emit)
    });
    assert_eq!(state.facts.agent, Some(CLIAgent::OhMyPi));

    // Minimized from a real omp /resume: replayed output contains its own
    // prompt/output block, but the enclosing shell command has not exited.
    (&mut state).with_facts(|facts, emit| {
        apply_signals(
            facts,
            sniffer.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;0\x07"),
            emit,
        )
    });
    assert_eq!(state.facts.agent, Some(CLIAgent::OhMyPi));
    assert_eq!(state.facts.shell.command.as_deref(), Some("omp"));
    (&mut state).with_facts(|facts, emit| {
        apply_signals(facts, sniffer.feed(b"\x1b]133;D;0;aid=lyrel\x07"), emit)
    });
    assert!(
        state.facts.agent.is_none(),
        "the actual shell exit releases the agent"
    );
}

#[test]
fn sniff_osc133_command_capture_drives_agent_detection() {
    let custom = std::collections::HashMap::new();
    let mut s = OscSniffer::new();

    let c = s.feed(b"\x1b]133;C;omp%20--help;aid=lyrel\x07");
    let shell = c.shell.last().unwrap();
    assert!(!shell.at_prompt);
    assert_eq!(shell.command.as_deref(), Some("omp --help"));
    assert_eq!(
        agent_from_shell_mark(shell, &custom),
        Some((
            crate::core::cli_agent::CLIAgent::OhMyPi,
            vec!["omp".to_string(), "--help".to_string()],
        ))
    );

    let d = s.feed(b"\x1b]133;D;0;aid=lyrel\x07");
    let shell = d.shell.last().unwrap();
    assert_eq!(shell.command, None);
    assert_eq!(agent_from_shell_mark(shell, &custom), None);

    let c = s.feed(b"\x1b]133;C;git%20status;aid=lyrel\x07");
    let shell = c.shell.last().unwrap();
    assert_eq!(shell.command.as_deref(), Some("git status"));
    assert_eq!(agent_from_shell_mark(shell, &custom), None);

    let c = s.feed(b"\x1b]133;C;;aid=lyrel\x07");
    assert_eq!(c.shell.last().unwrap().command, None);

    let _ = s.feed(b"\x1b]133;C;pi;aid=lyrel\x07");
    let a = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    assert_eq!(a.shell.last().unwrap().command.as_deref(), Some("pi"));
    let d = s.feed(b"\x1b]133;D;0;aid=lyrel\x07");
    assert_eq!(d.shell.last().unwrap().command, None);

    let c = s.feed(b"\x1b]133;C;echo%20a%0Aecho%20b;aid=lyrel\x07");
    assert_eq!(
        c.shell.last().unwrap().command.as_deref(),
        Some("echo a\necho b")
    );

    let c = s.feed(b"\x1b]133;C;%20%20;aid=lyrel\x07");
    assert_eq!(c.shell.last().unwrap().command, None);
}

#[test]
fn kimi_late_hooks_cannot_reopen_a_completed_durable_main_turn() {
    use crate::core::cli_agent::{AgentStatus, CLIAgent, parse_agent_event_body};
    let mut state = test_state(true);
    state.facts.agent = Some(CLIAgent::Kimi);
    let start = parse_agent_event_body(br#"{"agent":"kimi","event":"session-start","session_id":"s","session_scope":{"session_file":"/isolated/s/state.json"}}"#).unwrap();
    let ended = parse_agent_event_body(br#"{"agent":"kimi","event":"stop","session_id":"s","source":"kimi-durable-wire","requests":[],"result":{"outcome":"succeeded"}}"#).unwrap();
    (&mut state)
        .with_facts(|facts, emit| apply_agent_signals(facts, vec![start, ended], None, emit));
    let snapshot = state.facts.agent_session.clone().unwrap();
    let late = parse_agent_event_body(br#"{"agent":"kimi","event":"permission-request","session_id":"s","request":{"id":"late-p","kind":"permission","resolved":false}}"#).unwrap();
    (&mut state).with_facts(|facts, emit| apply_agent_signals(facts, vec![late], None, emit));
    assert_eq!(
        state.facts.agent_session.as_ref().unwrap().status,
        AgentStatus::Done
    );
    assert_eq!(state.facts.agent_session.unwrap(), snapshot);
}

#[test]
fn a_pane_killed_with_its_agent_keeps_the_facts_a_resume_needs() {
    use crate::core::cli_agent::{AgentSessionState, CLIAgent};
    use crate::core::machine::{
        AgentFacts, MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
        withdraw_observations,
    };

    const PANE: u64 = 77;
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(
            ws.id,
            None,
            PaneSeed {
                pane: PANE,
                cwd: Some("/work/api".to_string()),
                ssh_spec: None,
                agent: Some(AgentFacts {
                    agent: CLIAgent::OhMyPi,
                    session_id: Some("sess-1".to_string()),
                    launch_argv: Some(vec!["omp".to_string()]),
                    session_scope: None,
                    status: None,
                }),
                shell: None,
            },
            None,
            None,
        )
        .unwrap();
    publish_observations(&store);

    let run = |shutting_down: bool| {
        let mut state = test_state(true);
        state.id = PANE;
        state.facts.agent = Some(CLIAgent::OhMyPi);
        state.facts.agent_session = Some(AgentSessionState {
            session_id: Some("sess-1".to_string()),
            ..Default::default()
        });
        DaemonPane::spawn_reader(
            Arc::new(Mutex::new(state)),
            Arc::new(AtomicBool::new(shutting_down)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(b"\x1b]133;D;0\x07".to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| Some(None)),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();
    };

    run(true);
    let kept = store
        .pane(PANE)
        .expect("the record outlives the pane")
        .agent
        .expect("a teardown must not report the agent away");
    assert_eq!(kept.session_id.as_deref(), Some("sess-1"));

    run(false);
    assert!(
        store.pane(PANE).unwrap().agent.is_none(),
        "an agent that left a pane still in use is a fact, and clears"
    );

    withdraw_observations();
}

#[test]
fn an_explicit_agent_exit_clears_the_live_and_reopened_machine_snapshot() {
    use crate::core::cli_agent::{AgentSessionState, CLIAgent};
    use crate::core::machine::{
        MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
        withdraw_observations,
    };

    const PANE: u64 = 79;
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    let store = MachineStore::open(&path);
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(ws.id, None, PaneSeed::bare(PANE), None, None)
        .unwrap();
    publish_observations(&store);

    let mut state = test_state(true);
    state.id = PANE;
    state.facts.agent = Some(CLIAgent::OhMyPi);
    state.facts.agent_argv = Some(vec!["omp".into()]);
    state.facts.agent_session = Some(AgentSessionState {
        session_id: Some("explicitly-exited-session".into()),
        launch_argv: state.facts.agent_argv.clone(),
        ..Default::default()
    });
    publish_facts(PANE, true, observed_facts(&(&state).facts));
    store.flush();
    assert!(
        MachineStore::open(&path)
            .pane(PANE)
            .unwrap()
            .agent
            .is_some()
    );

    let state = Arc::new(Mutex::new(state));
    let stream = concat!(
        "\x1b]777;notify;lyrel://cli-agent;",
        r#"{"agent":"omp","event":"session-end","session_id":"explicitly-exited-session"}"#,
        "\x07",
    );
    DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(stream.as_bytes().to_vec())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            // Unsupported or unavailable process probes must not be needed
            // to honor the agent's own explicit shutdown event.
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(|| {})),
    )
    .join()
    .unwrap();
    store.flush();
    let saved_agent = MachineStore::open(&path).pane(PANE).unwrap().agent;
    let live_agent = store.pane(PANE).unwrap().agent;
    withdraw_observations();

    assert!(
        saved_agent.is_none(),
        "an explicitly exited agent must not be restored after restarting the daemon"
    );
    assert!(live_agent.is_none(), "the live machine tree must clear too");
    let mut state = state.lock().unwrap();
    assert!(state.facts.agent.is_none());
    assert!(state.facts.agent_session.is_none());
    assert!(state.facts.agent_argv.is_none());
    assert!(state.facts.agent_state(state.id).is_none());
    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut state, tx);
    assert!(!rx.try_iter().any(|msg| matches!(
        msg,
        DaemonMsg::Agent(Some(_)) | DaemonMsg::AgentStatus(Some(_))
    )));
}

#[test]
fn a_late_shutdown_from_the_previous_session_does_not_clear_the_current_agent() {
    use crate::core::cli_agent::{CLIAgent, parse_agent_event_body};

    let mut state = test_state(true);
    (&mut state).with_facts(|facts, emit| {
        apply_agent(facts, Some((CLIAgent::OhMyPi, vec!["omp".into()])), emit)
    });
    let start = parse_agent_event_body(
        br#"{"agent":"omp","event":"session-start","session_id":"current-session"}"#,
    )
    .unwrap();
    (&mut state).with_facts(|facts, emit| apply_agent_signals(facts, vec![start], None, emit));
    let before = state.facts.agent_session.clone();
    let ended = parse_agent_event_body(
        br#"{"agent":"omp","event":"session-end","session_id":"previous-session"}"#,
    )
    .unwrap();
    (&mut state).with_facts(|facts, emit| apply_agent_signals(facts, vec![ended], None, emit));
    assert_eq!(state.facts.agent, Some(CLIAgent::OhMyPi));
    assert_eq!(state.facts.agent_session, before);
}

#[test]
fn sentinel_events_drive_agent_session_state() {
    use crate::core::cli_agent::{AgentStatus, CLIAgent};

    let mut st = test_state(true);
    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);
    drain(&rx);

    let mut sniffer = OscSniffer::new();
    let stream = concat!(
        "\x1b]777;notify;lyrel://cli-agent;",
        r#"{"v":1,"agent":"omp","event":"session-start","session_id":"sid-9"}"#,
        "\x07",
        "\x1b]777;notify;lyrel://cli-agent;",
        r#"{"v":1,"agent":"omp","event":"prompt-submit"}"#,
        "\x07",
    );
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(stream.as_bytes()), emit));

    assert_eq!(st.facts.agent, Some(CLIAgent::OhMyPi));
    let sess = st
        .facts
        .agent_session
        .clone()
        .expect("session state exists");
    assert_eq!(sess.status, AgentStatus::Working);
    assert_eq!(sess.session_id.as_deref(), Some("sid-9"));
    assert!(sess.rich);

    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::Agent(Some(CLIAgent::OhMyPi)))
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::AgentStatus(Some(s))) if s.status == AgentStatus::Working
    ));

    let waiting = concat!(
        "\x1b]777;notify;lyrel://cli-agent;",
        r#"{"agent":"omp","event":"permission-request","message":"OhMyPi needs your permission to use Bash","request":{"id":"tool-9","kind":"permission"}}"#,
        "\x07",
    );
    (&mut st)
        .with_facts(|facts, emit| apply_signals(facts, sniffer.feed(waiting.as_bytes()), emit));
    assert_eq!(
        st.facts.agent_session.as_ref().unwrap().status,
        AgentStatus::Waiting
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::AgentStatus(Some(s))) if s.message.as_deref().unwrap().contains("permission")
    ));

    (&mut st).with_facts(|facts, emit| apply_agent(facts, None, emit));
    assert!(st.facts.agent_session.is_none());
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::AgentStatus(None))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Agent(None))));
}

#[test]
fn a_foreign_agent_hook_does_not_replace_the_foreground_session() {
    use crate::core::cli_agent::CLIAgent;

    let mut st = test_state(true);
    let mut sniffer = OscSniffer::new();
    let omp = concat!(
        "\x1b]777;notify;lyrel://cli-agent;",
        r#"{"v":1,"agent":"omp","event":"session-start","session_id":"01a0d21f-2f79-72e0-bb16-d6d908aaa6e0"}"#,
        "\x07",
    );
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(omp.as_bytes()), emit));
    assert_eq!(st.facts.agent, Some(CLIAgent::OhMyPi));

    let pi = concat!(
        "\x1b]777;notify;lyrel://cli-agent;",
        r#"{"v":1,"agent":"pi","event":"session-start","session_id":"e18a20ff-4c8c-4b94-867b-dd4b79032a6c"}"#,
        "\x07",
    );
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(pi.as_bytes()), emit));
    assert_eq!(
        st.facts
            .agent_session
            .as_ref()
            .unwrap()
            .session_id
            .as_deref(),
        Some("01a0d21f-2f79-72e0-bb16-d6d908aaa6e0"),
        "a Pi hook on an omp pane must not steal the session id"
    );

    (&mut st).with_facts(|facts, emit| {
        apply_agent(facts, Some((CLIAgent::Pi, vec!["pi".into()])), emit)
    });
    assert!(
        st.facts.agent_session.is_none(),
        "switching the foreground agent drops the previous session"
    );
}

#[test]
fn a_kitty_notification_marks_a_hookless_agent_waiting() {
    use crate::core::cli_agent::{AgentStatus, CLIAgent};

    let mut st = test_state(true);
    st.facts.agent = Some(CLIAgent::OhMyPi);
    let mut sniffer = OscSniffer::new();
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(
        b"\x1b]99;i=7:d=0:p=title;Oh My Pi\x07\x1b]99;i=7:p=body;OhMyPi needs your permission\x07\x1b]99;i=7:d=1:a=focus;\x07",
    ), emit));
    let sess = st.facts.agent_session.clone().unwrap();
    assert_eq!(sess.status, AgentStatus::Waiting);
    assert_eq!(
        sess.message.as_deref(),
        Some("OhMyPi needs your permission")
    );
}

#[test]
fn agent_state_snapshot_reports_only_panes_with_a_session() {
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let mut st = test_state(true);
    st.id = 42;
    assert_eq!(st.facts.agent_state(st.id), None);

    st.facts.agent = Some(CLIAgent::OhMyPi);
    assert_eq!(
        st.facts.agent_state(st.id),
        None,
        "a detected agent without session state is not yet a fact worth listing"
    );

    st.facts.agent_session = Some(AgentSessionState {
        status: AgentStatus::Waiting,
        session_id: Some("sess-1".into()),
        ..Default::default()
    });
    let snap = (&st)
        .facts
        .agent_state((&st).id)
        .expect("a session state is the fact");
    assert_eq!(snap.pane_id, 42);
    assert_eq!(snap.agent, Some(CLIAgent::OhMyPi));
    assert_eq!(snap.state.status, AgentStatus::Waiting);
    assert_eq!(snap.state.session_id.as_deref(), Some("sess-1"));
}
#[test]
fn a_hook_report_counts_only_from_inside_the_pane() {
    let (tx, rx) = mpsc::channel();
    let pane = DaemonPane::spawn(
        1,
        Some(PathBuf::from("/")),
        ws(80, 24),
        Some(ShellSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "exec cat".into()],
            args_are_lyrel_defaults: false,
        }),
        None,
        None,
        None,
        false,
        || {},
    )
    .expect("spawn pane");
    pane.attach(tx);
    let shell = pane.pty().and_then(|p| p.shell_pid).expect("shell pid");
    let body = r#"{"v":1,"agent":"omp","event":"prompt-submit","session_id":"s-1"}"#;

    let outsider = pane.report_agent_event(std::process::id(), body);
    let garbled = pane.report_agent_event(shell, "{not json");
    let applied = pane.report_agent_event(shell, body);
    let mut status = None;
    while let Ok(msg) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
        if let DaemonMsg::AgentStatus(s) = msg {
            status = s;
        }
    }
    pane.kill();

    assert!(
        outsider.is_err(),
        "the test runner does not run in the pane"
    );
    assert!(garbled.is_err());
    assert_eq!(applied, Ok(()));
    let status = status.expect("the report reached the pane's subscribers");
    assert_eq!(status.status, crate::core::cli_agent::AgentStatus::Working);
    assert_eq!(status.session_id.as_deref(), Some("s-1"));
}
