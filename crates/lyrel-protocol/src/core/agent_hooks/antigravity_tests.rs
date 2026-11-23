use super::*;

fn target<'a>(host: &'a dyn Host, home: &Path) -> HookTarget<'a> {
    let home = home.to_string_lossy().into_owned();
    HookTarget::local_for_exe(host, std::env::current_exe().unwrap())
        .expect("local hook target")
        .with_launch_arguments(
            HookAgent::Antigravity,
            &[
                format!("USERPROFILE={home}"),
                format!("HOME={home}"),
                "agy".into(),
                "--model".into(),
                "test-model".into(),
            ],
        )
}

fn registry(home: &Path) -> PathBuf {
    home.join(".gemini").join("config").join("hooks.json")
}

fn stdin(event: &str, rest: &str) -> String {
    let common = format!(
        "\"conversationId\":\"conv-1\",\"workspacePaths\":[\"/ws\"],\"transcriptPath\":\"/store/conv-1/transcript.jsonl\",\"artifactDirectoryPath\":\"/store/conv-1\",\"modelName\":\"gemini-4\",\"lastUserInput\":\"do not forward me\",\"hookEventName\":\"{event}\",\"cwd\":\"/ws\""
    );
    if rest.is_empty() {
        format!("{{{common}}}")
    } else {
        format!("{{{common},{rest}}}")
    }
}

fn report(arg: &str, payload: &str) -> crate::core::cli_agent::AgentEvent {
    let event = antigravity::effective(arg, payload).expect("the event reports");
    let body = build_hook_body("antigravity", event, payload);
    crate::core::cli_agent::parse_agent_event_body(body.as_bytes()).expect("the report parses")
}

fn apply(
    state: &mut crate::core::cli_agent::AgentSessionState,
    upstream: &str,
    rest: &str,
) -> crate::core::cli_agent::AgentStatus {
    let payload = stdin(upstream, rest);
    let event = report(upstream, &payload);
    state.apply_event(&event);
    state.status
}

#[test]
fn antigravity_registry_lifecycle_preserves_user_entries_and_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let host = crate::host::local::LocalHost::new();
    let target = target(&*host, dir.path());
    let path = registry(dir.path());
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::NotInstalled
    );

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"user-hook": {"enabled": true, "Stop": [{"command": "/bin/user-stop"}]},
            "lyrel": {"enabled": false}}"#,
    )
    .unwrap();
    assert!(
        install_hooks(&target, HookAgent::Antigravity).is_err(),
        "a lyrel-named entry carrying no lyrel commands is never rewritten"
    );

    let ours = format!(
        r#"{{"user-hook": {{"enabled": true, "Stop": [{{"command": "/bin/user-stop"}}],
                "PostToolUse": [{{"matcher": "run_command", "hooks": [{{"command": "./lint.sh"}}]}}]}},
            "lyrel": {{"enabled": false,
                "Stop": [{{"command": "\"{}\" agent-hook antigravity Stop"}}]}}}}"#,
        target.exe.to_string_lossy().replace('\\', "\\\\")
    );
    std::fs::write(&path, &ours).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::Outdated
    );
    install_hooks(&target, HookAgent::Antigravity).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::Installed
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let entry = &parsed["lyrel"];
    assert_eq!(
        entry["enabled"],
        serde_json::json!(false),
        "a user-set enabled flag survives install"
    );
    for event in ["SessionStart", "PreInvocation", "PostInvocation", "Stop"] {
        let handlers = entry[event].as_array().expect("a flat handler list");
        assert_eq!(handlers.len(), 1, "{event}");
        assert!(
            handlers[0]["command"]
                .as_str()
                .unwrap()
                .contains(&format!("agent-hook antigravity {event}")),
            "{event} invokes the lyrel runner"
        );
    }
    let grouped = &entry["PostToolUse"][0];
    assert_eq!(
        grouped["matcher"], "*",
        "tool events keep the grouped matcher shape (upstream #925)"
    );
    assert_eq!(grouped["hooks"][0]["type"], "command");
    let sibling = &parsed["user-hook"];
    assert_eq!(sibling["Stop"][0]["command"], "/bin/user-stop");
    assert_eq!(
        sibling["PostToolUse"][0]["hooks"][0]["command"],
        "./lint.sh"
    );

    install_hooks(&target, HookAgent::Antigravity).unwrap();
    uninstall_hooks(&target, HookAgent::Antigravity).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(parsed.get("lyrel").is_none());
    assert_eq!(parsed["user-hook"]["Stop"][0]["command"], "/bin/user-stop");
    assert_eq!(
        parsed["user-hook"]["PostToolUse"][0]["hooks"][0]["command"],
        "./lint.sh"
    );
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::NotInstalled
    );
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Antigravity).unwrap(),
        HookOutcome::NoLyrelHooks
    );

    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Antigravity).unwrap(),
        HookOutcome::NothingInstalled
    );
}

#[test]
fn antigravity_registry_rejects_foreign_lyrel_and_invalid_schema() {
    let dir = tempfile::tempdir().unwrap();
    let host = crate::host::local::LocalHost::new();
    let target = target(&*host, dir.path());
    let path = registry(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    let foreign = r#"{"lyrel": {"Stop": [{"command": "/bin/false"}]}}"#;
    std::fs::write(&path, foreign).unwrap();
    assert!(install_hooks(&target, HookAgent::Antigravity).is_err());
    assert!(uninstall_hooks(&target, HookAgent::Antigravity).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), foreign);

    std::fs::write(&path, "not json").unwrap();
    assert!(install_hooks(&target, HookAgent::Antigravity).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
}

#[test]
fn antigravity_events_report_only_authoritative_states() {
    use crate::core::cli_agent::{AgentEventKind, AgentOutcome, AgentSessionState, AgentStatus};

    assert_eq!(
        antigravity::effective("Stop", &stdin("Stop", "\"fullyIdle\": true")),
        Some("stop")
    );
    assert_eq!(
        antigravity::effective("Stop", &stdin("Stop", "\"fullyIdle\": false")),
        Some("prompt-submit"),
        "an unsettled stop keeps Working"
    );
    assert_eq!(
        antigravity::effective(
            "Stop",
            &stdin("Stop", "\"terminationReason\": \"NO_TOOL_CALL\"")
        ),
        Some("prompt-submit"),
        "absent fullyIdle is never an inferred Done"
    );
    assert_eq!(
        antigravity::effective("PreToolUse", &stdin("PreToolUse", "\"stepIdx\": 1")),
        None,
        "ordinary tool calls never report anything"
    );
    assert_eq!(antigravity::effective("PreToolUse", "{}"), None);
    assert_eq!(
        antigravity::effective("SessionStart", &stdin("SessionStart", "")),
        Some("session-start")
    );
    assert_eq!(
        antigravity::effective("PostInvocation", &stdin("PostInvocation", "")),
        Some("prompt-submit")
    );
    assert_eq!(
        antigravity::effective("PostToolUse", &stdin("PostToolUse", "\"stepIdx\": 2")),
        Some("tool-complete")
    );

    let mut state = AgentSessionState::default();
    assert_eq!(apply(&mut state, "SessionStart", ""), AgentStatus::Idle);
    assert_eq!(
        apply(&mut state, "PreInvocation", "\"invocationNum\": 0"),
        AgentStatus::Working
    );
    assert_eq!(
        apply(&mut state, "PostToolUse", "\"stepIdx\": 1"),
        AgentStatus::Working
    );
    assert_eq!(
        apply(&mut state, "PostInvocation", "\"invocationNum\": 0"),
        AgentStatus::Working
    );
    assert_eq!(
        apply(
            &mut state,
            "Stop",
            "\"executionNum\": 1, \"terminationReason\": \"NO_TOOL_CALL\", \"fullyIdle\": false"
        ),
        AgentStatus::Working,
        "background work still running is not Done"
    );
    assert_eq!(state.turns, 0);
    assert_eq!(
        apply(
            &mut state,
            "Stop",
            "\"executionNum\": 1, \"terminationReason\": \"NO_TOOL_CALL\", \"fullyIdle\": true"
        ),
        AgentStatus::Done
    );
    assert_eq!(state.turns, 1);
    assert!(
        state.readout.result.is_none(),
        "NO_TOOL_CALL supplies no verdict"
    );

    assert_eq!(
        apply(
            &mut state,
            "Stop",
            "\"executionNum\": 2, \"terminationReason\": \"ERROR\", \"error\": \"model exploded\", \"fullyIdle\": true"
        ),
        AgentStatus::Done
    );
    assert_eq!(state.turns, 2);
    let result = state.readout.result.as_ref().expect("an ERROR is a result");
    assert_eq!(result.outcome, AgentOutcome::Failed);
    assert_eq!(result.reason.as_deref(), Some("model exploded"));

    assert_eq!(
        apply(
            &mut state,
            "Stop",
            "\"executionNum\": 3, \"terminationReason\": \"USER_CANCELED\", \"fullyIdle\": true"
        ),
        AgentStatus::Done
    );
    assert_eq!(
        state.readout.result.as_ref().unwrap().outcome,
        AgentOutcome::Interrupted
    );
    assert_eq!(state.status, AgentStatus::Done);
    assert_ne!(state.status, AgentStatus::Waiting);

    let started = report("SessionStart", &stdin("SessionStart", ""));
    assert_eq!(started.kind, AgentEventKind::SessionStart);
    assert_eq!(started.session_id.as_deref(), Some("conv-1"));
    assert_eq!(started.cwd.as_deref(), Some(std::path::Path::new("/ws")));
    assert_eq!(started.readout.model.as_deref(), Some("gemini-4"));
    let body = build_hook_body("antigravity", "session-start", &stdin("SessionStart", ""));
    assert!(
        !body.contains("do not forward me"),
        "the typed prompt never rides along"
    );
}

#[test]
fn antigravity_hook_responses_are_always_passive() {
    assert_eq!(
        antigravity::response("PreToolUse"),
        r#"{"decision":"allow"}"#
    );
    assert_eq!(antigravity::response("Stop"), r#"{"decision":"allow"}"#);
    for event in [
        "SessionStart",
        "PreInvocation",
        "PostInvocation",
        "PostToolUse",
        "anything-else",
        "",
    ] {
        assert_eq!(antigravity::response(event), "{}", "{event}");
    }
    assert_eq!(
        antigravity::upstream_event("SessionStart", &stdin("Stop", "\"fullyIdle\":true")),
        "Stop",
        "a moved handler reports the event it actually ran under"
    );
    assert_eq!(
        antigravity::upstream_event("SessionStart", "not json"),
        "SessionStart"
    );
}
