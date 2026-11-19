//! Compare LocalHost with the shipped stdio server (or the explicitly provided
//! remote command) over an isolated OpenCode service.

#![cfg(unix)]

use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use lyrel_core::core::cli_agent::CLIAgent;
use lyrel_core::daemon::control::{ControlHello, LinkShutdown};
use lyrel_core::host::SharedHost;
use lyrel_core::host::local::LocalHost;
use lyrel_core::host::remote::RemoteHost;
use lyrel_core::host::session_operations::{SessionOperation, SessionOperationKind, SessionScope};
use lyrel_insights::statistics::{self, DateWindow, Source, StatisticsCache};

struct ServerProcess {
    child: Mutex<Option<Child>>,
}

impl LinkShutdown for ServerProcess {
    fn shutdown_link(&self) -> io::Result<()> {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

fn stdio_host(sandbox: &std::path::Path) -> SharedHost {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
        .args(["--stdio", "--serve"])
        .env("LYREL_DATA_DIR", sandbox)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not start lyrel-server --stdio");
    let stdout = child.stdout.take().expect("piped");
    let stdin = child.stdin.take().expect("piped");
    let closer: Arc<dyn LinkShutdown> = Arc::new(ServerProcess {
        child: Mutex::new(Some(child)),
    });
    let hello = ControlHello::host_rpc("opencode-statistics-smoke", "localhost");
    RemoteHost::connect_with(stdout, stdin, Some(closer), "stdio:opencode", &hello)
        .expect("handshake with lyrel-server --stdio")
        .into_shared()
}

#[test]
#[ignore = "requires isolated OpenCode 2.0.32 service and explicit LYREL_TEST_OPENCODE_* metering fixture"]
fn opencode_statistics_real_service_local_and_remote_host_agree() {
    let executable = std::env::var("LYREL_TEST_OPENCODE_EXECUTABLE").unwrap();
    let home = std::env::var("LYREL_TEST_OPENCODE_HOME").unwrap();
    let server = std::env::var("LYREL_TEST_OPENCODE_SERVER").unwrap_or_default();
    let expected_tokens: u64 = std::env::var("LYREL_TEST_OPENCODE_TOKENS")
        .unwrap()
        .parse()
        .unwrap();
    let expected_sessions: u64 = std::env::var("LYREL_TEST_OPENCODE_SESSIONS")
        .unwrap()
        .parse()
        .unwrap();
    let expected_cost: f64 = std::env::var("LYREL_TEST_OPENCODE_COST")
        .unwrap()
        .parse()
        .unwrap();
    let sandbox = tempfile::tempdir().unwrap();
    let remote = if let Ok(command) = std::env::var("LYREL_TEST_OPENCODE_REMOTE_COMMAND") {
        let mut child = Command::new("sh")
            .args(["-c", &command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let closer: Arc<dyn LinkShutdown> = Arc::new(ServerProcess {
            child: Mutex::new(Some(child)),
        });
        let hello = ControlHello::host_rpc("opencode-statistics-smoke", "localhost");
        RemoteHost::connect_with(
            stdout,
            stdin,
            Some(closer),
            "isolated-ssh-statistics",
            &hello,
        )
        .unwrap()
        .into_shared()
    } else {
        stdio_host(sandbox.path())
    };
    let local = LocalHost::new();
    for host in [&*local as &dyn lyrel_core::host::Host, &*remote] {
        let mut scope = SessionScope::default();
        scope.environment.insert("HOME".into(), home.clone());
        scope
            .environment
            .insert("XDG_CONFIG_HOME".into(), format!("{home}/config"));
        scope
            .environment
            .insert("XDG_DATA_HOME".into(), format!("{home}/data"));
        scope
            .environment
            .insert("XDG_STATE_HOME".into(), format!("{home}/state"));
        scope
            .environment
            .insert("XDG_CACHE_HOME".into(), format!("{home}/cache"));
        let mut launch_argv = vec![executable.clone()];
        if !server.is_empty() {
            launch_argv.extend(["--server".into(), server.clone()]);
        }
        let request = SessionOperation {
            expected_host: host.id().0,
            agent: CLIAgent::OpenCode,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: home.clone().into(),
            launch_argv,
            scope,
        };
        let source = Source::opencode(home.clone().into(), "isolated-real-service", request);
        let snapshot = statistics::scan(host, &[source], &mut StatisticsCache::default()).unwrap();
        let totals = snapshot.aggregate(&DateWindow::all("UTC"));
        assert!(snapshot.is_complete(), "{:?}", snapshot.issues());
        assert_eq!(totals.tokens().total(), Some(expected_tokens));
        assert_eq!(totals.main_sessions(), expected_sessions);
        assert_eq!(totals.recorded_usd(), Some(expected_cost));
        eprintln!(
            "real OpenCode Host={:?}: tokens={:?}, main={}, cost={:?}",
            host.id(),
            totals.tokens().total(),
            totals.main_sessions(),
            totals.recorded_usd()
        );
    }
}
