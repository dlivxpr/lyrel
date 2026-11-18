use super::*;

#[test]
fn hermes_safe_mode_does_not_start_a_fake_fork() {
    let dir = tempfile::tempdir().unwrap();
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Fork,
        source_id: "20261018_193219_a2659f".into(),
        cwd: dir.path().into(),
        launch_argv: vec!["hermes".into(), "chat".into(), "--safe-mode".into()],
        scope: SessionScope::default(),
    };
    let error = crate::host::local::LocalHost::new()
        .session_operation(&request)
        .unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Unsupported);
    assert!(error.retained.is_none());
}

#[test]
fn hermes_profile_mismatch_cannot_fork_in_another_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut scope = SessionScope {
        profile: Some("work".into()),
        ..Default::default()
    };
    scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Fork,
        source_id: "20261018_193219_a2659f".into(),
        cwd: dir.path().into(),
        launch_argv: vec!["hermes".into(), "chat".into()],
        scope,
    };
    let error = crate::host::local::LocalHost::new()
        .session_operation(&request)
        .unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Identity);
    assert!(error.retained.is_none());
}

/// Exercises the selected installation without touching the user's Hermes home.
#[test]
#[ignore = "requires LYREL_TEST_HERMES pointing to an installed Hermes launcher"]
fn native_hermes_history_uses_selected_launcher_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let mut scope = SessionScope::default();
    scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: dir.path().into(),
        launch_argv: vec![std::env::var("LYREL_TEST_HERMES").unwrap()],
        scope,
    };
    let sessions = crate::host::local::LocalHost::new()
        .hermes_sessions(&request)
        .expect("the selected Hermes installation must expose native history");
    assert!(
        sessions.is_empty(),
        "an isolated home has no stored sessions"
    );
}

/// Reopen an existing recovery target without ever creating another fork.
#[test]
#[ignore = "requires LYREL_TEST_HERMES, LYREL_TEST_HERMES_HOME, LYREL_TEST_HERMES_SESSION and LYREL_TEST_HERMES_CWD"]
fn native_hermes_resume_releases_gateway_and_preserves_stored_session() {
    let host = crate::host::local::LocalHost::new();
    let mut request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: std::env::var("LYREL_TEST_HERMES_SESSION").unwrap(),
        cwd: PathBuf::from(std::env::var("LYREL_TEST_HERMES_CWD").unwrap()),
        launch_argv: vec![
            std::env::var("LYREL_TEST_HERMES").unwrap(),
            "chat".into(),
            "--cli".into(),
        ],
        scope: SessionScope::default(),
    };
    request.scope.environment.insert(
        "HERMES_HOME".into(),
        std::env::var("LYREL_TEST_HERMES_HOME").unwrap(),
    );
    let before = host.hermes_sessions(&request).unwrap();
    assert!(before.iter().any(|session| session.id == request.source_id));
    for _ in 0..2 {
        let resumed = host.session_operation(&request).unwrap();
        assert_eq!(
            resumed.target_id.as_deref(),
            Some(request.source_id.as_str())
        );
        assert!(!resumed.created);
        assert!(
            resumed
                .command
                .as_ref()
                .unwrap()
                .contains(&request.source_id)
        );
        assert_eq!(
            resumed.scope.environment.get("HERMES_HOME"),
            request.scope.environment.get("HERMES_HOME")
        );
    }
    let after = host.hermes_sessions(&request).unwrap();
    assert!(after.iter().any(|session| session.id == request.source_id));
    let mut before_ids = before.iter().map(|session| &session.id).collect::<Vec<_>>();
    let mut after_ids = after.iter().map(|session| &session.id).collect::<Vec<_>>();
    before_ids.sort();
    after_ids.sort();
    assert_eq!(
        before_ids, after_ids,
        "Resume must not create a recovery fork"
    );
}

/// Real official source-install launcher + native gateway, explicitly isolated by the caller.
#[test]
#[ignore = "requires LYREL_TEST_HERMES, LYREL_TEST_HERMES_HOME, LYREL_TEST_HERMES_SESSION and LYREL_TEST_HERMES_CWD"]
fn native_hermes_fork_retains_a_stored_identity_and_independent_source() {
    let host = crate::host::local::LocalHost::new();
    let mut request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Fork,
        source_id: std::env::var("LYREL_TEST_HERMES_SESSION").unwrap(),
        cwd: PathBuf::from(std::env::var("LYREL_TEST_HERMES_CWD").unwrap()),
        launch_argv: vec![
            std::env::var("LYREL_TEST_HERMES").unwrap(),
            "chat".into(),
            "--cli".into(),
        ],
        scope: SessionScope::default(),
    };
    request.scope.environment.insert(
        "HERMES_HOME".into(),
        std::env::var("LYREL_TEST_HERMES_HOME").unwrap(),
    );
    let history = host.hermes_sessions(&request).unwrap();
    let source = history
        .iter()
        .find(|session| session.id == request.source_id)
        .expect("native source is visible in the same home");
    assert_eq!(source.agent, CLIAgent::Hermes);
    assert_eq!(source.cwd.as_deref(), Some(request.cwd.as_path()));
    assert_eq!(
        source
            .scope
            .as_ref()
            .unwrap()
            .environment
            .get("HERMES_HOME"),
        request.scope.environment.get("HERMES_HOME")
    );
    let fork = host.session_operation(&request).unwrap();
    let target = fork.target_id.clone().unwrap();
    assert_ne!(target, request.source_id);
    assert!(fork.created);
    assert!(fork.command.as_ref().unwrap().contains(&target));
    let retained = fork
        .opening_failed("pane connection failed")
        .retained
        .unwrap();
    assert_eq!(retained.target_id.as_deref(), Some(target.as_str()));
    assert!(retained.created);
    request.kind = SessionOperationKind::Resume;
    let original = host.session_operation(&request).unwrap();
    assert_eq!(
        original.target_id.as_deref(),
        Some(request.source_id.as_str())
    );
    request.source_id = target.clone();
    let reopened = host.session_operation(&request).unwrap();
    assert_eq!(reopened.target_id.as_deref(), Some(target.as_str()));
    assert!(!reopened.created);
    let history = host.hermes_sessions(&request).unwrap();
    assert!(history.iter().any(|session| session.id == target));
    let mut other = request.clone();
    other.expected_host = 999;
    assert_eq!(
        host.hermes_sessions(&other).unwrap_err().kind,
        SessionErrorKind::WrongHost
    );
    request.source_id = "lyrel-missing-session-must-not-create".into();
    for kind in [SessionOperationKind::Resume, SessionOperationKind::Fork] {
        request.kind = kind;
        let error = host.session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Command);
        assert!(error.retained.is_none());
    }
}

#[test]
fn hermes_statistics_rejects_wrong_host_and_profile_without_starting_runtime() {
    let host = crate::host::local::LocalHost::new();
    let dir = tempfile::tempdir().unwrap();
    let mut request = SessionOperation {
        expected_host: 999,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: dir.path().into(),
        launch_argv: vec!["hermes".into()],
        scope: SessionScope::default(),
    };
    assert_eq!(
        host.hermes_statistics(&request).unwrap_err().kind,
        SessionErrorKind::WrongHost
    );
    request.expected_host = 0;
    request.scope.profile = Some("work".into());
    request.scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    assert_eq!(
        host.hermes_statistics(&request).unwrap_err().kind,
        SessionErrorKind::Identity
    );
}

#[cfg(windows)]
#[test]
fn hermes_history_preserves_result_while_owned_gateway_finishes_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let gateway = dir.path().join("gateway.ps1");
    std::fs::write(
        &gateway,
        r#"$ErrorActionPreference = 'Stop'
[void][Console]::ReadLine()
[Console]::WriteLine('{"jsonrpc":"2.0","id":"lyrel-hermes-operation","result":{"projects":[{"previewSessions":[{"id":"cleanup-session","source":"cli","title":"Stored history","last_active":1}]}]}}')
[Console]::Out.Flush()
[void][Console]::In.ReadToEnd()
Start-Sleep -Seconds 6
[IO.File]::WriteAllText((Join-Path $env:HERMES_HOME 'gateway-cleanup'), 'flushed')
"#,
    )
    .unwrap();
    let launcher = dir.path().join("hermes.cmd");
    // Build the runtime JSON in PowerShell, so Unicode home paths do not pass through cmd's code page.
    std::fs::write(
        &launcher,
        "@echo off\r\n\"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoProfile -NonInteractive -Command \"[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false); [Console]::WriteLine((ConvertTo-Json -InputObject @((Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'), '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $env:HERMES_HOME 'gateway.ps1')) -Compress))\"\r\n",
    )
    .unwrap();
    let mut scope = SessionScope::default();
    scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: dir.path().into(),
        launch_argv: vec![launcher.to_string_lossy().into_owned()],
        scope,
    };
    let sessions = crate::host::local::LocalHost::new()
        .hermes_sessions(&request)
        .expect("valid stored history must survive cleanup within the operation deadline");
    assert_eq!(sessions[0].id, "cleanup-session");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("gateway-cleanup")).unwrap(),
        "flushed",
        "the owned gateway must finish its cleanup instead of being killed after its valid response"
    );
}
