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

#[test]
#[ignore = "requires isolated LYREL_TEST_HERMES and LYREL_TEST_HERMES_HOME"]
fn native_hermes_statistics_reads_public_stored_usage() {
    use crate::statistics::{self, DateWindow, StatisticsCache};
    let host = crate::host::local::LocalHost::new();
    let home = PathBuf::from(std::env::var("LYREL_TEST_HERMES_HOME").unwrap());
    let launcher = std::env::var("LYREL_TEST_HERMES").unwrap();
    let launch = std::collections::HashMap::from([(
        "hermes".into(),
        format!("HERMES_HOME='{}' '{}'", home.display(), launcher),
    )]);
    let sources = statistics::discover_hermes(&*host, &launch).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].root(), home.as_path());
    let snapshot = statistics::scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.main_sessions(), 2);
    assert_eq!(totals.tokens().input(), Some(17));
    assert_eq!(totals.tokens().output(), Some(8));
    assert_eq!(totals.recorded_usd(), Some(0.0));
    assert_eq!(totals.estimated_usd(), Some(1.0));
    assert!(!snapshot.is_complete());
    assert_eq!(
        snapshot.issues().len(),
        1,
        "unexpected gateway/runtime failure: {:?}",
        snapshot.issues()
    );
    let mut scope = SessionScope::default();
    scope
        .environment
        .insert("HERMES_HOME".into(), home.to_string_lossy().into_owned());
    let payload = host
        .hermes_statistics(&SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Hermes,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: home.clone(),
            launch_argv: vec![launcher],
            scope,
        })
        .unwrap();
    for row in payload["projects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["previewSessions"].as_array().unwrap())
    {
        assert!(
            row.get("preview").is_none(),
            "statistics transport must not carry prompt previews"
        );
        assert!(
            row.get("title").is_none(),
            "statistics transport does not need conversation titles"
        );
    }
    let omp = tempfile::tempdir().unwrap();
    let timestamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(omp.path().join("native.jsonl"), format!(
        "{{\"type\":\"session\",\"id\":\"main\",\"timestamp\":\"{timestamp}\"}}\n{{\"type\":\"model_usage\",\"id\":\"stored:main\",\"timestamp\":\"{timestamp}\",\"usage\":{{\"input\":1,\"output\":1,\"totalTokens\":2,\"cost\":{{\"total\":0}}}}}}\n"
    )).unwrap();
    let mut mixed = sources;
    mixed.push(statistics::Source::omp(
        omp.path().into(),
        mixed[0].scope().to_owned(),
    ));
    let snapshot = statistics::scan(&*host, &mixed, &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(
        totals.main_sessions(),
        3,
        "same IDs and scope strings from different agents remain independent"
    );
    assert_eq!(totals.tokens().total(), Some(27));
}

#[test]
#[ignore = "requires official LYREL_TEST_HERMES and isolated LYREL_TEST_HERMES_HOME containing 502 native CLI sessions"]
fn native_hermes_statistics_marks_502_session_history_as_windowed() {
    use crate::statistics::{self, DateWindow, StatisticsCache};
    let host = crate::host::local::LocalHost::new();
    let home = PathBuf::from(std::env::var("LYREL_TEST_HERMES_HOME").unwrap());
    let launcher = std::env::var("LYREL_TEST_HERMES").unwrap();
    let launch = std::collections::HashMap::from([(
        "hermes".into(),
        format!("HERMES_HOME='{}' '{}'", home.display(), launcher),
    )]);
    let sources = statistics::discover_hermes(&*host, &launch).unwrap();
    let snapshot = statistics::scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.main_sessions(), 501);
    assert_eq!(totals.tokens().total(), Some(0));
    assert!(!snapshot.is_complete());
    assert!(
        snapshot
            .issues()
            .iter()
            .any(|i| i.reason().starts_with("Hermes global 501-session")),
        "{:?}",
        snapshot.issues()
    );
}

#[test]
#[ignore = "requires isolated SSH server argv, official native Hermes launcher and seeded work profile in LYREL_HERMES_SSH_*"]
fn native_hermes_statistics_real_ssh_host_preserves_profile_and_disconnects() {
    use crate::daemon::control::{ControlHello, LinkShutdown};
    use crate::host::remote::RemoteHost;
    use crate::statistics::{self, DateWindow, StatisticsCache};
    use std::sync::{Arc, Mutex};
    struct Process(Mutex<Option<std::process::Child>>);
    impl LinkShutdown for Process {
        fn shutdown_link(&self) -> io::Result<()> {
            if let Some(mut child) = self.0.lock().unwrap().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            Ok(())
        }
    }
    let argv: Vec<String> =
        serde_json::from_str(&std::env::var("LYREL_HERMES_SSH_ARGV").unwrap()).unwrap();
    let home = std::env::var("LYREL_HERMES_SSH_HOME").unwrap();
    let launcher = std::env::var("LYREL_HERMES_SSH_LAUNCHER").unwrap();
    assert!(home.ends_with("/profiles/work"));
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let closer: Arc<dyn LinkShutdown> = Arc::new(Process(Mutex::new(Some(child))));
    let remote = RemoteHost::connect_with(
        stdout,
        stdin,
        Some(closer.clone()),
        "hermes-isolated-ssh",
        &ControlHello::host_rpc("hermes-statistics-smoke", "isolated-ssh"),
    )
    .unwrap();
    assert!(
        remote
            .peer()
            .has_feature(crate::daemon::control::feature::HERMES_STATISTICS)
    );
    let host = remote.into_shared();
    assert!(!host.id().is_local());
    let launch = std::collections::HashMap::from([(
        "hermes".into(),
        format!("HERMES_HOME='{home}' '{launcher}'"),
    )]);
    let sources = statistics::discover_hermes(&*host, &launch).unwrap();
    assert_eq!(sources[0].root(), Path::new(&home));
    let mut cache = StatisticsCache::default();
    let snapshot = statistics::scan(&*host, &sources, &mut cache).unwrap();
    assert_eq!(snapshot.host(), host.id().0);
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.main_sessions(), 2);
    assert_eq!(totals.tokens().total(), Some(25));
    assert_eq!(totals.recorded_usd(), Some(0.0));
    assert_eq!(totals.estimated_usd(), Some(1.0));
    assert!(!snapshot.is_complete());
    assert_eq!(snapshot.issues().len(), 1, "{:?}", snapshot.issues());
    let mut scope = SessionScope {
        profile: Some("default".into()),
        ..Default::default()
    };
    scope.environment.insert("HERMES_HOME".into(), home.clone());
    let mut request = SessionOperation {
        expected_host: host.id().0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: home.into(),
        launch_argv: vec![launcher],
        scope,
    };
    assert_eq!(
        host.hermes_statistics(&request).unwrap_err().kind,
        SessionErrorKind::Identity
    );
    request.scope.profile = Some("work".into());
    closer.shutdown_link().unwrap();
    assert_eq!(
        host.hermes_statistics(&request).unwrap_err().kind,
        SessionErrorKind::Connection
    );
    match statistics::scan(&*host, &sources, &mut cache) {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::NotConnected),
        Ok(stale) => {
            assert!(!stale.is_complete());
            assert_eq!(
                stale.aggregate(&DateWindow::all("UTC")).tokens().total(),
                Some(25)
            );
        }
    }
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
