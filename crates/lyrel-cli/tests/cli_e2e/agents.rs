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
                program: std::env::current_exe().unwrap().display().to_string(),
                args: vec![FIXTURE_ARG.into()],
                args_are_lyrel_defaults: false,
            }),
            Some("agent-age-e2e".into()),
            None,
        )
        .unwrap();
    session.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
    collect_pane_output_until(&mut session, b"AGENT_AGE_READY");
    let pane = session.pane_id();
    let address = format!("%{pane}");
    let state = || {
        let report = daemon.run_json(&["agents"]);
        report["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["pane_id"].as_u64() == Some(pane))
            .expect("real OSC hooks reach the CLI agent list")
            .get("state")
            .unwrap()
            .clone()
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
