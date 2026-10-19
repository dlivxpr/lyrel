use super::*;

#[test]
fn hermes_installation_preserves_consent_and_user_files() {
    // The production Host reads its process environment. Run this consumer in
    // its own process instead of mutating shared test environment or touching
    // an ambient HERMES_HOME supplied by a developer.
    if std::env::var_os("LYREL_TEST_HERMES_MANAGEMENT_CHILD").is_none() {
        let home = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "core::agent_hooks::hermes_tests::hermes_installation_preserves_consent_and_user_files", "--nocapture"])
            .env("HERMES_HOME", home.path())
            .env("LYREL_TEST_HERMES_MANAGEMENT_CHILD", "1")
            .status().unwrap();
        assert!(status.success());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let host = crate::host::local::LocalHost::new();
    let target = HookTarget {
        host: &*host,
        home: dir.path().into(),
        exe: PathBuf::from("lyrel-app"),
        launch_scopes: Default::default(),
    };
    let root = hermes::home(&target).unwrap();
    let config = target.under(&root, &["config.yaml"]);
    target
        .write(&config, b"plugins:\n  enabled: []\nuser_setting: keep\n")
        .unwrap();
    let before = target.read(&config).unwrap();
    install_hooks(&target, HookAgent::Hermes).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Hermes),
        HooksState::Installed
    );
    assert_eq!(target.read(&config).unwrap(), before);
    assert_eq!(
        hooks_diagnostic(&target, HookAgent::Hermes),
        Some(HooksDiagnostic::HermesObserver { home: root.clone() })
    );
    let user_file = target.under(&root, &["plugins", "lyrel", "user.txt"]);
    target.write(&user_file, b"keep").unwrap();
    uninstall_hooks(&target, HookAgent::Hermes).unwrap();
    assert_eq!(target.read(&config).unwrap(), before);
    assert_eq!(target.read(&user_file).unwrap(), "keep");
    let user_plugin = target.under(&root, &["plugins", "lyrel", "__init__.py"]);
    target
        .write(&user_plugin, b"# user-managed plugin; do not overwrite")
        .unwrap();
    assert!(install_hooks(&target, HookAgent::Hermes).is_err());
    assert!(uninstall_hooks(&target, HookAgent::Hermes).is_err());
    assert_eq!(
        target.read(&user_plugin).unwrap(),
        "# user-managed plugin; do not overwrite"
    );
    let profile = target.under(&root, &["profiles", "work"]);
    let profile_config = target.host.join(&profile, "config.yaml");
    target
        .write(&profile_config, b"user_setting: profile-preserved\n")
        .unwrap();
    let selector = target.host.join(&root, "active_profile");
    target.write(&selector, b"\xef\xbb\xbfwork\n").unwrap();
    install_hooks(&target, HookAgent::Hermes).unwrap();
    assert_eq!(hermes::home(&target).unwrap(), profile);
    assert_eq!(
        hooks_state(&target, HookAgent::Hermes),
        HooksState::Installed
    );
    uninstall_hooks(&target, HookAgent::Hermes).unwrap();
    assert_eq!(
        target.read(&profile_config).unwrap(),
        "user_setting: profile-preserved\n"
    );
    assert_eq!(
        target.read(&user_plugin).unwrap(),
        "# user-managed plugin; do not overwrite"
    );
    let configured = HookTarget {
        host: &*host,
        home: dir.path().into(),
        exe: PathBuf::from("lyrel-app"),
        launch_scopes: Default::default(),
    }
    .with_launch_commands(&std::collections::HashMap::from([(
        "hermes".into(),
        "hermes chat --profile default --model team --cli".into(),
    )]));
    assert_eq!(hermes::home(&configured).unwrap(), root);
    assert!(
        install_hooks(&configured, HookAgent::Hermes).is_err(),
        "explicit default does not write to the sticky named profile or overwrite user code"
    );
    let tombstone = target.under(&root, &["profiles", ".deleted", "work"]);
    target.write(&tombstone, b"deleted\n").unwrap();
    assert!(install_hooks(&target, HookAgent::Hermes).is_err());
    assert!(matches!(
        hooks_diagnostic(&target, HookAgent::Hermes),
        Some(HooksDiagnostic::ScopeUnavailable { .. })
    ));
}

#[test]
fn hermes_identity_scope_cannot_leak_credentials() {
    let body = build_hook_body(
        "hermes",
        "session-start",
        r#"{"session_id":"20261018_193219_a2659f","session_scope":{"profile":"work","environment":{"HERMES_HOME":"/isolated/work","OPENAI_API_KEY":"secret"}}}"#,
    );
    let event = crate::core::cli_agent::parse_agent_event_body(body.as_bytes()).unwrap();
    assert_eq!(event.session_id.as_deref(), Some("20261018_193219_a2659f"));
    let scope = event.readout.session_scope.unwrap();
    assert_eq!(scope.profile.as_deref(), Some("work"));
    assert_eq!(
        scope.environment.get("HERMES_HOME").map(String::as_str),
        Some("/isolated/work")
    );
    assert!(!body.contains("secret"));
}

#[test]
fn hermes_requests_keep_independent_waits_and_failure_result() {
    use crate::core::cli_agent::{
        AgentOutcome, AgentSessionState, AgentStatus, parse_agent_event_body,
    };
    let mut state = AgentSessionState::default();
    for (event, payload, status, pending) in [
        (
            "session-start",
            r#"{"session_id":"main"}"#,
            AgentStatus::Idle,
            0,
        ),
        (
            "prompt-submit",
            r#"{"session_id":"main"}"#,
            AgentStatus::Working,
            0,
        ),
        (
            "permission-request",
            r#"{"request":{"id":"approve-a","kind":"permission"}}"#,
            AgentStatus::Waiting,
            1,
        ),
        (
            "permission-request",
            r#"{"request":{"id":"clarify-b","kind":"question"}}"#,
            AgentStatus::Waiting,
            2,
        ),
        (
            "tool-complete",
            r#"{"request":{"id":"approve-a","kind":"permission","resolved":true}}"#,
            AgentStatus::Waiting,
            1,
        ),
        (
            "tool-complete",
            r#"{"request":{"id":"clarify-b","kind":"question","resolved":true}}"#,
            AgentStatus::Working,
            0,
        ),
        (
            "stop",
            r#"{"session_id":"main","result":{"outcome":"failed","reason":"max_iterations"}}"#,
            AgentStatus::Done,
            0,
        ),
    ] {
        let body = build_hook_body("hermes", event, payload);
        state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        assert_eq!(state.status, status, "{event}");
        assert_eq!(state.pending_requests.len(), pending, "{event}");
        assert_eq!(state.session_id.as_deref(), Some("main"));
    }
    assert_eq!(
        state.readout.result.as_ref().unwrap().outcome,
        AgentOutcome::Failed
    );
    assert_eq!(
        state.readout.result.as_ref().unwrap().reason.as_deref(),
        Some("max_iterations")
    );
}

#[test]
#[ignore = "requires LYREL_TEST_HERMES_PYTHON; controlled observer callbacks are not real Hermes acceptance"]
fn hermes_production_observer_preserves_waits_compression_and_terminal_outcomes() {
    use crate::core::cli_agent::{
        AgentOutcome, AgentSessionState, AgentStatus, parse_agent_event_body,
    };
    let output = std::process::Command::new(std::env::var("LYREL_TEST_HERMES_PYTHON").unwrap())
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/core/agent_hooks/hermes_observer_fixture.py"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let expected = [
        AgentStatus::Idle,
        AgentStatus::Working,
        AgentStatus::Waiting,
        AgentStatus::Waiting,
        AgentStatus::Waiting,
        AgentStatus::Working,
        AgentStatus::Working,
        AgentStatus::Done,
        AgentStatus::Working,
        AgentStatus::Done,
        AgentStatus::Working,
        AgentStatus::Done,
        AgentStatus::Working,
        AgentStatus::Working,
        AgentStatus::Done,
    ];
    assert_eq!(events.len(), expected.len());
    let mut state = AgentSessionState::default();
    let mut outcomes = Vec::new();
    for (body, expected) in events.iter().zip(expected) {
        let event = parse_agent_event_body(&serde_json::to_vec(body).unwrap()).unwrap();
        state.apply_event(&event);
        assert_eq!(state.status, expected, "{body}");
        if state.status == AgentStatus::Done {
            outcomes.push(state.readout.result.as_ref().unwrap().outcome);
        }
    }
    assert_eq!(
        outcomes,
        [
            AgentOutcome::Succeeded,
            AgentOutcome::Interrupted,
            AgentOutcome::Failed,
            AgentOutcome::Succeeded
        ]
    );
    assert_eq!(state.session_id.as_deref(), Some("compressed-main"));
    assert!(
        events
            .iter()
            .any(|event| event["result"]["outcome"] == "failed"
                && event["result"]["reason"] == "max_iterations")
    );
}
