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
