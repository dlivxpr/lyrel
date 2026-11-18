//! Production server/client coverage: no GUI process or pane tree is sampled.
use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lyrel_core::daemon::control::{ControlClient, ControlHello, LinkShutdown};
use lyrel_core::host::metrics::MetricsState;

struct ServerProcess {
    child: Mutex<Option<Child>>,
    #[cfg(windows)]
    daemon: WindowsDaemon,
}

impl LinkShutdown for ServerProcess {
    fn shutdown_link(&self) -> io::Result<()> {
        if let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(windows)]
        self.daemon.stop();
        Ok(())
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.shutdown_link();
    }
}

fn diagnostics(profile: &std::path::Path) -> String {
    ["metrics-stdio.log", "metrics-daemon.log"]
        .iter()
        .map(|name| {
            format!(
                "{name}: {}",
                std::fs::read_to_string(profile.join(name)).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn stdio_connection(
    profile: &std::path::Path,
) -> (
    std::process::ChildStdout,
    std::process::ChildStdin,
    Arc<dyn LinkShutdown>,
) {
    // Unix can own a real control server in this process. Windows only bridges
    // stdio, so explicitly start an isolated daemon first, exactly as remote
    // Windows installation does. Never let a bridge auto-discover a user daemon.
    #[cfg(windows)]
    let daemon = WindowsDaemon::start(profile);
    let mut command = Command::new(env!("CARGO_BIN_EXE_lyrel-server"));
    #[cfg(unix)]
    command.args(["--stdio", "--serve"]);
    #[cfg(windows)]
    command
        .args(["--stdio", "--bridge", "--control-sock"])
        .arg(profile.join("control.port"));
    let mut child = command
        .env("LYREL_DATA_DIR", profile)
        .env("LYREL_CONFIG_DIR", profile)
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_SOCKET")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_PANE")
        .env_remove("LYREL_SHELL_INTEGRATION")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(profile.join("metrics-stdio.log")).unwrap())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let shutdown: Arc<dyn LinkShutdown> = Arc::new(ServerProcess {
        child: Mutex::new(Some(child)),
        #[cfg(windows)]
        daemon,
    });
    (stdout, stdin, shutdown)
}

#[test]
fn a_real_daemon_reports_its_whole_host_and_stops_answering_after_disconnect() {
    let profile = tempfile::tempdir().unwrap();
    let (stdout, stdin, closer) = stdio_connection(profile.path());
    let client = ControlClient::connect_with(
        stdout,
        stdin,
        Some(closer),
        &ControlHello::host_rpc("metrics-test", "consumer"),
        Box::new(|_| {}),
    )
    .unwrap_or_else(|error| {
        panic!(
            "metrics control handshake failed: {error}\n{}",
            diagnostics(profile.path())
        )
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = client.host_metrics().unwrap();
    assert!(
        matches!(first.state(), MetricsState::Waiting),
        "a new daemon needs a CPU baseline first"
    );
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
                assert!(
                    Instant::now() < deadline,
                    "host never completed its initial sample"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
            MetricsState::Unavailable(reason) => panic!("native host sampling failed: {reason}"),
        }
    }
    client.close();
    assert!(
        client.host_metrics().is_err(),
        "a dead daemon must not yield cached/local data"
    );
}

fn remote(profile: &std::path::Path, key: &str) -> Arc<lyrel_core::host::remote::RemoteHost> {
    let (stdout, stdin, shutdown) = stdio_connection(profile);
    lyrel_core::host::remote::RemoteHost::connect_with(
        stdout,
        stdin,
        Some(shutdown),
        key,
        &ControlHello::host_rpc("remote-metrics", "consumer"),
    )
    .unwrap_or_else(|error| {
        panic!(
            "remote Host handshake failed: {error}\n{}",
            diagnostics(profile)
        )
    })
}

fn remote_sample(host: &dyn lyrel_core::host::Host) -> lyrel_core::host::metrics::HostMetrics {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let metrics = host.host_metrics().unwrap();
        match metrics.state() {
            MetricsState::Sample(sample) => {
                assert!((0.0..=100.0).contains(&sample.cpu_percent()));
                assert!(sample.memory_total_bytes() > 0);
                assert!(sample.memory_used_bytes() <= sample.memory_total_bytes());
                return metrics;
            }
            MetricsState::Waiting => {
                assert!(
                    Instant::now() < deadline,
                    "remote CPU never finished its baseline"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
            MetricsState::Unavailable(reason) => panic!("remote host sampling failed: {reason}"),
        }
    }
}

#[test]
fn remote_metrics_keep_their_daemon_identity_across_hosts_disconnect_and_reconnect() {
    use lyrel_core::host::{Host as _, HostId};
    let profile_a = tempfile::tempdir().unwrap();
    let profile_b = tempfile::tempdir().unwrap();
    let a = remote(profile_a.path(), "metrics-host-a");
    let b = remote(profile_b.path(), "metrics-host-b");
    assert_ne!(a.id(), HostId::LOCAL);
    assert_ne!(a.id(), b.id());
    let first_a = remote_sample(&*a);
    let first_b = remote_sample(&*b);
    assert_eq!(first_a.instance(), a.peer().instance);
    assert_eq!(first_b.instance(), b.peer().instance);
    assert_ne!(
        first_a.instance(),
        first_b.instance(),
        "the client sampled itself instead of the selected daemon"
    );
    assert!(!first_a.hostname().is_empty());
    assert!(!first_b.hostname().is_empty());
    a.client().close();
    assert!(
        a.host_metrics().is_err(),
        "a disconnected remote must never yield a local/cached sample"
    );
    assert_eq!(
        remote_sample(&*b).instance(),
        first_b.instance(),
        "another host was disturbed by A disconnecting"
    );
    let reconnected = remote(profile_a.path(), "metrics-host-a");
    assert_eq!(a.id(), reconnected.id());
    let next_a = remote_sample(&*reconnected);
    assert_eq!(next_a.instance(), reconnected.peer().instance);
    assert_ne!(
        next_a.instance(),
        first_a.instance(),
        "reconnect reused the dead daemon's measurement"
    );
    reconnected.client().close();
    b.client().close();
}

#[test]
fn a_peer_without_native_metrics_capability_is_unavailable_without_breaking_its_link() {
    use lyrel_core::host::Host as _;
    use std::net::{TcpListener, TcpStream};
    fn connect(
        host: lyrel_core::host::SharedHost,
        key: &str,
    ) -> (
        Arc<lyrel_core::host::remote::RemoteHost>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let thread =
            std::thread::spawn(move || lyrel_core::host::server::serve(server, host).unwrap());
        let host = lyrel_core::host::remote::RemoteHost::over_tcp(
            stream,
            key,
            &ControlHello::host_rpc("unsupported-metrics", "consumer"),
        )
        .unwrap();
        (host, thread)
    }
    let (native, native_thread) =
        connect(lyrel_core::host::local::LocalHost::new(), "native-metrics");
    // A server acting as a Host proxy serves real Host operations, but does not
    // advertise that it owns a native whole-machine sampler. No fake replies.
    let (proxy, proxy_thread) = connect(native.clone().into_shared(), "non-native-metrics");
    assert_eq!(
        proxy.host_metrics().unwrap_err().kind(),
        io::ErrorKind::Unsupported
    );
    proxy.client().ping().unwrap();
    assert!(proxy.client().is_connected());
    proxy.client().close();
    proxy_thread.join().unwrap();
    native.client().close();
    native_thread.join().unwrap();
}

#[cfg(windows)]
struct WindowsDaemon {
    child: Mutex<Option<Child>>,
    job: WindowsJob,
}

#[cfg(windows)]
impl WindowsDaemon {
    fn start(profile: &std::path::Path) -> Self {
        let job = WindowsJob::new();
        let child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
            .arg("--daemon")
            .arg("--config-dir")
            .arg(profile)
            .env("LYREL_CONFIG_DIR", profile)
            .env("LYREL_DATA_DIR", profile)
            .env_remove("LYREL_CONTROL_SOCK")
            .env_remove("LYREL_SOCKET")
            .env_remove("LYREL_WS")
            .env_remove("LYREL_PANE")
            .env_remove("LYREL_SHELL_INTEGRATION")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(profile.join("metrics-daemon.log")).unwrap())
            .spawn()
            .unwrap();
        let daemon = Self {
            child: Mutex::new(Some(child)),
            job,
        };
        {
            let child = daemon.child.lock().unwrap();
            daemon.job.assign(child.as_ref().unwrap());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let endpoint = profile.join("control.port");
            if lyrel_core::client::ControlClient::connect_at(
                &endpoint,
                &ControlHello::host_rpc("metrics-readiness", "probe"),
            )
            .is_ok()
            {
                return daemon;
            }
            let exited = daemon
                .child
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap();
            assert!(
                exited.is_none(),
                "isolated daemon exited with {exited:?}\n{}",
                diagnostics(profile)
            );
            assert!(
                Instant::now() < deadline,
                "isolated daemon never opened control.port\n{}",
                diagnostics(profile)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn stop(&self) {
        if let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

// Same kill-on-close ownership as CLI E2E: only this fixture's daemon and its
// children belong to the job. HANDLE can be used/closed across threads; the
// owning guard closes it only after its last shared ServerProcess is dropped.
#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for WindowsJob {}
#[cfg(windows)]
unsafe impl Sync for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    fn new() -> Self {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        let job = Self(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
        assert!(
            !job.0.is_null(),
            "CreateJobObjectW failed: {}",
            io::Error::last_os_error()
        );
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        assert_ne!(
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            },
            0,
            "SetInformationJobObject failed: {}",
            io::Error::last_os_error()
        );
        job
    }

    fn assign(&self, child: &Child) {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        assert_ne!(
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) },
            0,
            "AssignProcessToJobObject failed: {}",
            io::Error::last_os_error()
        );
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}
