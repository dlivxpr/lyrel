//! Native Hermes statistics cases that moved with `lyrel-insights`' statistics
//! owner. All three require explicit external fixtures (ignored by default,
//! exactly as they were in `lyrel-core`): a seeded Hermes home, a 502-session
//! Hermes history, or an isolated SSH server argv.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use lyrel_core::host::remote::RemoteHost;
use lyrel_insights::statistics::{self, DateWindow, Source, StatisticsCache};
use lyrel_protocol::core::cli_agent::CLIAgent;
use lyrel_protocol::daemon::control::{ControlHello, LinkShutdown, feature};
use lyrel_protocol::host::local::LocalHost;
use lyrel_protocol::host::session_operations::{
    SessionErrorKind, SessionOperation, SessionOperationKind, SessionScope,
};

#[test]
#[ignore = "requires isolated LYREL_TEST_HERMES and LYREL_TEST_HERMES_HOME"]
fn native_hermes_statistics_reads_public_stored_usage() {
    let host = LocalHost::new();
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
    mixed.push(Source::omp(omp.path().into(), mixed[0].scope().to_owned()));
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
    let host = LocalHost::new();
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
    assert!(remote.peer().has_feature(feature::HERMES_STATISTICS));
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
