use super::*;

pub(super) const FIXTURE_ARG: &str = "--lyrel-e2e-agent-age-fixture";

pub(super) fn fixture() {
    fn emit(events: &[serde_json::Value]) {
        let mut out = std::io::stdout().lock();
        for event in events {
            write!(out, "\x1b]777;notify;lyrel://cli-agent;{event}\x07").unwrap();
        }
        writeln!(out, "AGENT_AGE_READY").unwrap();
        out.flush().unwrap();
    }
    let event =
        |kind: &str| serde_json::json!({"agent":"omp", "event":kind, "session_id":"age-fixture"});
    let request = |id: &str, resolved: bool| {
        serde_json::json!({
            "agent":"omp", "event": if resolved { "tool-complete" } else { "permission-request" }, "session_id":"age-fixture",
            "request":{"id":id,"kind":"permission","resolved":resolved}
        })
    };
    emit(&[
        event("session-start"),
        event("prompt-submit"),
        request("first", false),
    ]);
    for line in std::io::stdin().lock().lines() {
        match line.unwrap().trim() {
            "repeat" => emit(&[request("first", false)]),
            "second" => emit(&[request("second", false)]),
            "stop-pending" => emit(&[event("stop")]),
            "resolve-first" => emit(&[request("first", true)]),
            "finish" => emit(&[request("second", true), event("stop")]),
            "batch" => emit(&[event("prompt-submit"), event("stop")]),
            _ => {}
        }
    }
}

pub(super) fn agents_report_continuous_state_age(daemon: &Daemon) {
    // Foreground probing must agree with the synthetic OMP hooks. An unnamed
    // test runner is correctly treated as a non-agent and its state cleared.
    let fixture_dir = tempfile::tempdir().unwrap();
    let executable = fixture_dir.path().join(if cfg!(windows) { "omp.exe" } else { "omp" });
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let mut session = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 100,
                rows: 30,
                cell_w: 8,
                cell_h: 16,
            },
            Some(ShellSpec {
                program: executable.display().to_string(),
                args: vec![FIXTURE_ARG.into()],
                args_are_lyrel_defaults: false,
            }),
            Some("agent-age-e2e".into()),
            None,
        )
        .unwrap();
    let deadline = Instant::now() + SETTLE_WITHIN;
    // Output precedes fact publication, but split reads may publish state
    // before the final marker. Observe both without depending on their order
    // or the timestamp whose semantics the assertions verify.
    let mut output = Vec::new();
    let mut waiting = false;
    while !waiting || !output.windows(b"AGENT_AGE_READY".len()).any(|bytes| bytes == b"AGENT_AGE_READY") {
        let remaining = deadline.checked_duration_since(Instant::now())
            .expect("the fixture must publish Waiting within the original deadline");
        session.set_recv_timeout(Some(remaining)).unwrap();
        match session.recv().expect("receive the fixture's committed agent state") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => output.extend_from_slice(&bytes),
            DaemonMsg::AgentStatus(Some(state)) => waiting = state.status == lyrel_core::core::cli_agent::AgentStatus::Waiting,
            DaemonMsg::AgentStatus(None) => waiting = false,
            DaemonMsg::Exited { code } => panic!("the agent fixture exited before readiness: {code:?}"),
            _ => {}
        }
    }
    session.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
    let pane = session.pane_id();
    let address = format!("%{pane}");
    let state = || {
        let report = daemon.run_json(&["agents"]);
        report["agents"].as_array().unwrap().iter()
            .find(|entry| entry["pane_id"].as_u64() == Some(pane))
            .unwrap_or_else(|| panic!("the identity-matched agent must remain listed; pane={pane}; report={report}"))
            .get("state").unwrap().clone()
    };
    let waiting = state();
    assert_eq!(waiting["status"], "waiting");
    let since = waiting["since_ms"]
        .as_u64()
        .expect("daemon stamps the continuous wait");
    assert!(since > 0);
    let table = daemon.run_ok(&["agents"]);
    assert_eq!(
        table
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["PANE", "AGENT", "STATUS", "FOR", "MESSAGE"]
    );
    for (command, pending) in [
        ("repeat", 1),
        ("second", 2),
        ("stop-pending", 2),
        ("resolve-first", 1),
    ] {
        std::thread::sleep(Duration::from_millis(20));
        daemon.run_ok(&["send", &address, command, "--enter"]);
        collect_pane_output_until(&mut session, b"AGENT_AGE_READY");
        let current = state();
        assert_eq!(current["status"], "waiting");
        assert_eq!(current["since_ms"], since);
        assert_eq!(
            current["pending_requests"].as_array().unwrap().len(),
            pending
        );
    }
    daemon.run_ok(&["send", &address, "finish", "--enter"]);
    collect_pane_output_until(&mut session, b"AGENT_AGE_READY");
    let done = state();
    assert_eq!(done["status"], "done");
    let done_since = done["since_ms"].as_u64().unwrap();
    assert!(done_since > since);
    std::thread::sleep(Duration::from_millis(20));
    daemon.run_ok(&["send", &address, "batch", "--enter"]);
    collect_pane_output_until(&mut session, b"AGENT_AGE_READY");
    let next = state();
    assert_eq!(next["status"], "done");
    assert!(next["since_ms"].as_u64().unwrap() > done_since);
    assert_eq!(next["turns"].as_u64(), Some(2));
    daemon.run_ok(&["pane", "close", &address]);
}
