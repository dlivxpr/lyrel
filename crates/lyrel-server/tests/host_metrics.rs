//! Production server/client coverage: no GUI process or pane tree is sampled.
use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lyrel_core::daemon::control::{ControlClient, ControlHello, LinkShutdown};
use lyrel_core::host::metrics::MetricsState;

struct ServerProcess(Mutex<Option<Child>>);
impl LinkShutdown for ServerProcess {
    fn shutdown_link(&self) -> io::Result<()> {
        if let Some(mut child) = self.0.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

#[test]
fn a_real_daemon_reports_its_whole_host_and_stops_answering_after_disconnect() {
    let profile = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
        .args(["--stdio", "--serve"])
        .env("LYREL_DATA_DIR", profile.path())
        .env("LYREL_CONFIG_DIR", profile.path())
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_SOCKET")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let closer: Arc<dyn LinkShutdown> = Arc::new(ServerProcess(Mutex::new(Some(child))));
    let client = ControlClient::connect_with(
        stdout, stdin, Some(closer), &ControlHello::host_rpc("metrics-test", "consumer"),
        Box::new(|_| {}),
    ).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = client.host_metrics().unwrap();
    assert!(matches!(first.state(), MetricsState::Waiting), "a new daemon needs a CPU baseline first");
    assert!(!first.hostname().is_empty());
    assert_eq!(first.instance(), client.hello().instance);
    let hostname = first.hostname().to_owned();
    loop {
        let metrics = client.host_metrics().unwrap();
        assert_eq!(metrics.hostname(), hostname);
        assert_eq!(metrics.instance(), client.hello().instance);
        match metrics.state() {
            MetricsState::Sample(sample) => {
                assert!(sample.cpu_percent().is_finite());
                assert!((0.0..=100.0).contains(&sample.cpu_percent()));
                assert!(sample.memory_total_bytes() > 0);
                assert!(sample.memory_used_bytes() <= sample.memory_total_bytes());
                assert!(sample.sampled_at_millis() > 0);
                break;
            }
            MetricsState::Waiting => {
                assert!(Instant::now() < deadline, "host never completed its initial sample");
                std::thread::sleep(Duration::from_millis(50));
            }
            MetricsState::Unavailable(reason) => panic!("native host sampling failed: {reason}"),
        }
    }
    client.close();
    assert!(client.host_metrics().is_err(), "a dead daemon must not yield cached/local data");
}

#[test]
fn a_remote_host_without_a_metrics_adapter_never_samples_the_client_machine() {
    use lyrel_core::host::{Host as _, HostId};
    use std::net::{TcpListener, TcpStream};
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    let serving = std::thread::spawn(move || {
        lyrel_core::host::server::serve(server, lyrel_core::host::local::LocalHost::new()).unwrap();
    });
    let remote = lyrel_core::host::remote::RemoteHost::over_tcp(stream, "metrics-unsupported-host",
        &ControlHello::host_rpc("remote-metrics-test", "consumer")).unwrap();
    assert_ne!(remote.id(), HostId::LOCAL);
    let unavailable = remote.host_metrics().unwrap_err();
    assert_eq!(unavailable.kind(), io::ErrorKind::Unsupported);
    assert!(remote.client().is_connected(), "unavailable metrics must not kill the control link");
    remote.client().close();
    serving.join().unwrap();
}
