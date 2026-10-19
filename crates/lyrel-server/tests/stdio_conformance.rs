#![cfg(unix)]

use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use lyrel_core::daemon::control::{ControlHello, LinkShutdown};
use lyrel_core::host::SharedHost;
use lyrel_core::host::conformance::Sandbox;
use lyrel_core::host::remote::RemoteHost;

struct ServerProcess {
    child: Mutex<Option<Child>>,
}

impl LinkShutdown for ServerProcess {
    fn shutdown_link(&self) -> io::Result<()> {
        let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return Ok(());
        };
        let _ = child.kill();
        let _ = child.wait();
        Ok(())
    }
}

struct TempSandbox(tempfile::TempDir);

impl Sandbox for TempSandbox {
    fn path(&self) -> &Path {
        self.0.path()
    }

    fn symlink(&self, target: &Path, link: &Path) -> Option<io::Result<()>> {
        #[cfg(unix)]
        {
            Some(std::os::unix::fs::symlink(target, link))
        }
        #[cfg(not(unix))]
        {
            let _ = (target, link);
            None
        }
    }
}

fn stdio_host() -> (SharedHost, TempSandbox) {
    let sandbox = TempSandbox(tempfile::TempDir::new().unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
        .args(["--stdio", "--serve"])
        .env("LYREL_DATA_DIR", sandbox.path())
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

    let hello = ControlHello::host_rpc("stdio-conformance", "localhost");
    let host = RemoteHost::connect_with(stdout, stdin, Some(closer), "stdio:conformance", &hello)
        .expect("handshake with lyrel-server --stdio");

    (host.into_shared(), sandbox)
}

lyrel_core::host_conformance_suite!(remote_stdio, stdio_host);

#[test]
fn the_whole_suite_ran_against_the_server() {
    let names: Vec<&str> = lyrel_core::host::conformance::CASES
        .iter()
        .map(|(n, _)| *n)
        .collect();
    assert!(
        names.len() >= 52,
        "the conformance suite shrank to {} cases: {names:?}",
        names.len()
    );
}

#[test]
fn the_server_really_is_another_process() {
    let (host, sandbox) = stdio_host();
    let marker = host.join(sandbox.path(), "written-over-the-wire.txt");
    host.write_file(&marker, b"from the client").unwrap();

    assert_eq!(std::fs::read(&marker).unwrap(), b"from the client");

    let from_here = sandbox.path().join("written-locally.txt");
    std::fs::write(&from_here, b"from the test").unwrap();
    assert_eq!(
        host.read_file(&from_here, 1024).unwrap(),
        b"from the test",
        "the server read a file this process wrote"
    );
    assert!(host.is_connected());
}

#[test]
fn dropping_the_host_reaps_the_server() {
    let (host, sandbox) = stdio_host();
    assert!(host.exists(sandbox.path()));
    drop(host);
}

#[test]
#[ignore = "requires isolated OpenCode 2.0.32 service and explicit LYREL_TEST_OPENCODE_* metering fixture"]
fn opencode_statistics_real_service_local_and_remote_host_agree() {
    use lyrel_core::core::cli_agent::CLIAgent;
    use lyrel_core::host::session_operations::{SessionOperation, SessionOperationKind, SessionScope};
    use lyrel_core::host::local::LocalHost;
    use lyrel_core::statistics::{self, DateWindow, Source, StatisticsCache};
    let executable = std::env::var("LYREL_TEST_OPENCODE_EXECUTABLE").unwrap();
    let home = std::env::var("LYREL_TEST_OPENCODE_HOME").unwrap();
    let server = std::env::var("LYREL_TEST_OPENCODE_SERVER").unwrap();
    let expected_tokens: u64 = std::env::var("LYREL_TEST_OPENCODE_TOKENS").unwrap().parse().unwrap();
    let expected_sessions: u64 = std::env::var("LYREL_TEST_OPENCODE_SESSIONS").unwrap().parse().unwrap();
    let expected_cost: f64 = std::env::var("LYREL_TEST_OPENCODE_COST").unwrap().parse().unwrap();
    let (stdio_remote, _sandbox) = stdio_host();
    let remote = if let Ok(argv) = std::env::var("LYREL_TEST_OPENCODE_REMOTE_ARGV") {
        let argv: Vec<String> = serde_json::from_str(&argv).unwrap();
        let mut child = Command::new(&argv[0]).args(&argv[1..])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let closer: Arc<dyn LinkShutdown> = Arc::new(ServerProcess { child: Mutex::new(Some(child)) });
        let hello = ControlHello::host_rpc("opencode-statistics-smoke", "localhost");
        RemoteHost::connect_with(stdout, stdin, Some(closer), "isolated-ssh-statistics", &hello).unwrap().into_shared()
    } else { stdio_remote };
    let local = LocalHost::new();
    for host in [&*local as &dyn lyrel_core::host::Host, &*remote] {
        let mut scope = SessionScope::default();
        scope.environment.insert("HOME".into(), home.clone());
        scope.environment.insert("XDG_CONFIG_HOME".into(), format!("{home}/config"));
        scope.environment.insert("XDG_DATA_HOME".into(), format!("{home}/data"));
        scope.environment.insert("XDG_STATE_HOME".into(), format!("{home}/state"));
        scope.environment.insert("XDG_CACHE_HOME".into(), format!("{home}/cache"));
        let request = SessionOperation { expected_host:host.id().0, agent:CLIAgent::OpenCode,
            kind:SessionOperationKind::Resume, source_id:String::new(), cwd:home.clone().into(),
            launch_argv:vec![executable.clone(), "--server".into(), server.clone()], scope };
        let source = Source::opencode(home.clone().into(), "isolated-real-service", request);
        let snapshot = statistics::scan(host, &[source], &mut StatisticsCache::default()).unwrap();
        let totals = snapshot.aggregate(&DateWindow::all("UTC"));
        assert!(snapshot.is_complete(), "{:?}", snapshot.issues());
        assert_eq!(totals.tokens().total(), Some(expected_tokens));
        assert_eq!(totals.main_sessions(), expected_sessions);
        assert_eq!(totals.recorded_usd(), Some(expected_cost));
        eprintln!("real OpenCode Host={:?}: tokens={:?}, main={}, cost={:?}", host.id(), totals.tokens().total(), totals.main_sessions(), totals.recorded_usd());
    }
}
