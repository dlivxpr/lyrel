// Shared real native-SSH fixture. No parent environment or installed daemon is changed.
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lyrel_core::client::{ControlClient, PaneClient};
use lyrel_core::daemon::control::{ControlHello, ControlRequest, ReplyOk, ServerStatus};
use lyrel_core::daemon::protocol::{
    self, ClientMsg, DaemonVersion, NativeSshSpec, ShellSpec, SshAuthMode, WinSize,
};
use lyrel_core::daemon::router::{self, RouteHeader};
use lyrel_core::daemon::transport;
use lyrel_core::host::Host;
use lyrel_core::host::remote::RemoteHost;

pub const STREAM_WITHIN: Duration = Duration::from_secs(90);

struct RestoreRouteAuth(Arc<dyn router::RouteAuthResponder>);

impl Drop for RestoreRouteAuth {
    fn drop(&mut self) { router::set_route_auth_responder(self.0.clone()); }
}

fn strict_route(stream: &mut transport::Stream, header: &RouteHeader) -> std::io::Result<router::RouteAck> {
    // Live-endpoint cases are run explicitly with --test-threads=1. Never let
    // an application-installed responder accept a changed fixture host key.
    let _restore = RestoreRouteAuth(router::route_auth_responder());
    router::set_route_auth_responder(Arc::new(router::CancelAuth));
    router::negotiate(stream, header)
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes { write!(result, "{byte:02x}").unwrap(); }
    result
}

pub fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    (0..text.len()).step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

pub fn win_size() -> WinSize {
    WinSize { cols: 80, rows: 24, cell_w: 10, cell_h: 20 }
}

fn required(name: &str) -> String {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("real native SSH prerequisite missing: {name}"))
}

pub fn status(host: &RemoteHost) -> ServerStatus {
    match host.client().call(ControlRequest::Status).expect("remote Status") {
        ReplyOk::Status(status) => status,
        other => panic!("expected remote Status, got {other:?}"),
    }
}

pub struct NativeSshFixture {
    child: Child,
    binary: PathBuf,
    root: tempfile::TempDir,
    spec: NativeSshSpec,
    remote_home: String,
    remote_dir: String,
    node: String,
    bundled: Option<String>,
    control: Mutex<Option<Arc<RemoteHost>>>,
    panes: Mutex<Vec<u64>>,
}

impl NativeSshFixture {
    pub fn start(binary: &Path) -> Self {
        assert!(binary.is_absolute() && binary.is_file(), "explicit production lyrel-server binary required");
        let host = required("LYREL_TEST_SSH_HOST");
        let port: u16 = required("LYREL_TEST_SSH_PORT").parse().expect("SSH port must be u16");
        assert_ne!(port, 0);
        let user = required("LYREL_TEST_SSH_USER");
        let identity = required("LYREL_TEST_SSH_IDENTITY");
        assert!(Path::new(&identity).is_absolute() && Path::new(&identity).is_file(), "explicit fixture identity file required");
        // This is a PUBLIC pin, never the private identity or host key.
        let pin_path = required("LYREL_TEST_SSH_KNOWN_HOSTS");
        let pin = std::fs::read_to_string(pin_path).expect("read fixture public host-key pin");
        let lines: Vec<_> = pin.lines().filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1, "fixture requires exactly one fixed host-key pin");
        let fields: Vec<_> = lines[0].split_whitespace().collect();
        assert!(fields.len() >= 3, "invalid fixture host-key pin");
        let token = if port == 22 { host.clone() } else { format!("[{host}]:{port}") };
        assert_eq!(fields[0], token, "pin must name precisely this SSH endpoint");
        assert_eq!(fields[1], "ssh-ed25519", "fixture requires its fixed ed25519 pin");
        let remote_home = required("LYREL_TEST_SSH_REMOTE_HOME");
        let node = required("LYREL_TEST_SSH_NODE");
        assert!(remote_home.starts_with('/') && node.starts_with('/'), "explicit Linux home and Node executable paths required");
        assert!(!remote_home.ends_with('/'));
        let root = tempfile::Builder::new().prefix("lyrel-ssh46-").tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("home/.ssh")).unwrap();
        std::fs::write(root.path().join("home/.ssh/known_hosts"), format!("{}\n", lines[0])).unwrap();
        let bundled = std::env::var("LYREL_TEST_SSH_BUNDLED_SERVER_DIR").ok();
        let spec = NativeSshSpec {
            host, port, user,
            auth_mode: SshAuthMode::PublicKey,
            identity_files: vec![identity],
            verify_host_keys: true,
            agent_forward: false,
            shell_integration: false,
            connect_timeout_s: Some(30),
            password: None,
            key_passphrases: None,
            proxy: lyrel_core::daemon::protocol::SshProxy::None,
            jump: None,
            forwards: Vec::new(),
            keepalive_interval_s: None,
            keepalive_count_max: None,
            algorithms: Default::default(),
            x11: false,
            term: "xterm-256color".into(),
            skip_banner: false,
            remote_clipboard_write: false,
            login_script: Vec::new(),
            display_name: None,
            profile_id: None,
        };
        let child = Self::launch(binary, root.path(), bundled.as_deref());
        let remote_dir = format!("{remote_home}/.{}", root.path().file_name().unwrap().to_str().unwrap());
        let fixture = Self {
            child, binary: binary.to_owned(), root, spec, remote_home, remote_dir, node, bundled,
            control: Mutex::new(None), panes: Mutex::new(Vec::new()),
        };
        fixture.await_ready();
        fixture
    }

    fn launch(binary: &Path, root: &Path, bundled: Option<&str>) -> Child {
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let log = std::fs::OpenOptions::new().create(true).append(true).open(root.join("router.log")).unwrap();
        let mut command = Command::new(binary);
        command.args(["--daemon", "--config-dir"]).arg(&config)
            .env("LYREL_CONFIG_DIR", &config)
            .env("LYREL_DATA_DIR", root.join("data"))
            .env("LYREL_CONTROL_SOCK", config.join("control.sock"))
            .env("HOME", root.join("home"))
            .env("USERPROFILE", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("home/.config"))
            .env_remove("SSH_AUTH_SOCK").env_remove("SSH_AGENT_PID")
            .env_remove("LYREL_PANE").env_remove("LYREL_WS")
            .env_remove("LYREL_BUNDLED_SERVER_DIR")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
        if let Some(bundled) = bundled { command.env("LYREL_BUNDLED_SERVER_DIR", bundled); }
        command.spawn().expect("start isolated production local daemon router")
    }

    fn endpoint(&self, name: &str) -> PathBuf {
        self.root.path().join("config").join(format!("{name}.{}", if cfg!(windows) { "port" } else { "sock" }))
    }

    fn await_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let control = ControlClient::connect_at(&self.endpoint("control"), &ControlHello::host_rpc("ssh46-local-probe", "fixture"));
            if control.is_ok() && PaneClient::at(self.endpoint("daemon")).version().is_ok() { return; }
            assert!(Instant::now() < deadline, "isolated local router not ready: {}", std::fs::read_to_string(self.root.path().join("router.log")).unwrap_or_default());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn spec(&self) -> NativeSshSpec { self.spec.clone() }
    pub fn remote_home(&self) -> &str { &self.remote_home }

    pub fn remote_path(&self, name: &str) -> PathBuf {
        assert!(!name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..");
        // A Linux path is opaque here. Only RemoteHost may resolve/access it.
        PathBuf::from(format!("{}/{name}", self.remote_dir))
    }

    fn route(&self, pane: bool) -> transport::Stream {
        let mut stream = transport::connect_endpoint_at(&self.endpoint("daemon")).expect("explicit local router endpoint");
        stream.set_read_timeout(Some(STREAM_WITHIN)).unwrap();
        stream.set_write_timeout(Some(STREAM_WITHIN)).unwrap();
        let header = RouteHeader::ssh(self.spec());
        let header = if pane { header.for_pane() } else { header };
        let ack = strict_route(&mut stream, &header).expect("strict pinned native SSH route (no interactive trust/auth fallback)");
        assert!(ack.ok);
        assert!(matches!(ack.link.as_deref(), Some("streamlocal" | "session-exec")), "not a real SSH route: {ack:?}");
        stream
    }

    pub fn connect_control(&self) -> Arc<RemoteHost> {
        let stream = self.route(false);
        let hello = ControlHello::host_rpc("native-ssh-graphics-46", "isolated-fixture");
        #[cfg(windows)]
        let host = RemoteHost::over_tcp(stream, "fixture:native-ssh-46", &hello).unwrap();
        #[cfg(unix)]
        let host = RemoteHost::over_unix(stream, "fixture:native-ssh-46", &hello).unwrap();
        assert_eq!(host.peer().home, self.remote_home, "remote control HOME must be the fixture SFTP/shell home");
        assert_eq!(host.peer().separator, '/');
        assert_eq!(host.peer().protocol_version, protocol::PROTOCOL_VERSION);
        assert!(!host.peer().instance.is_empty());
        let remote = status(&host);
        assert_ne!(remote.pid, 0);
        assert_eq!(remote.protocol_version, protocol::PROTOCOL_VERSION);
        assert_eq!(remote.control_version, lyrel_core::daemon::control::CONTROL_VERSION);
        *self.control.lock().unwrap() = Some(host.clone());
        self.assert_sftp_home();
        host
    }

    fn assert_sftp_home(&self) {
        use lyrel_core::daemon::protocol::{DaemonMsg, SftpOp, SftpOpResult, WorkspaceOp, WorkspaceRequest};
        // The workspace SFTP command reuses the SSH engine connection held by
        // the real local router. Even quick_prove's hot-server shortcut cannot
        // bypass this independent check of SFTP Realpath(".").
        let mut stream = transport::connect_endpoint_at(&self.endpoint("daemon")).unwrap();
        stream.set_read_timeout(Some(STREAM_WITHIN)).unwrap();
        stream.set_write_timeout(Some(STREAM_WITHIN)).unwrap();
        protocol::negotiate_pane(&mut stream).unwrap();
        ClientMsg::OnWorkspace(Box::new(WorkspaceRequest {
            workspace: lyrel_core::core::session::WorkspaceId::new(),
            spec: Box::new(self.spec()),
            view_pane: 0,
            op: WorkspaceOp::SftpOp { op: SftpOp::Realpath { path: ".".into() } },
        })).encode(&mut stream).unwrap();
        match DaemonMsg::read(&mut stream).expect("real native SSH workspace SFTP response") {
            DaemonMsg::SftpOpResult(SftpOpResult::Link(home)) => assert_eq!(home, self.remote_home, "SFTP and control/shell HOME must share the private Linux fixture namespace"),
            other => panic!("real SFTP home proof failed: {other:?}"),
        }
    }

    pub fn connect_pane(&self) -> (transport::Stream, DaemonVersion) {
        self.connect_pane_with_capacity(64 << 20, 128 << 20)
    }

    pub fn connect_pane_with_capacity(&self, encoded: u64, decoded: u64) -> (transport::Stream, DaemonVersion) {
        assert!(encoded <= 64 << 20 && decoded <= 128 << 20);
        let mut stream = self.route(true);
        let version = protocol::negotiate_pane(&mut stream).expect("remote pane dialect negotiation");
        assert!(!version.instance.is_empty());
        ClientMsg::GraphicsCapacity { encoded_bytes: encoded, decoded_bytes: decoded, offers: true }.encode(&mut stream).unwrap();
        (stream, version)
    }

    pub fn install_sender(&self, host: &RemoteHost) -> ShellSpec {
        let directory = PathBuf::from(&self.remote_dir);
        host.create_dir(&directory, false).expect("create unique private remote fixture directory");
        let script = self.remote_path("kitty_remote_sender.mjs");
        let body = include_bytes!("kitty_remote_sender.mjs");
        host.write_file(&script, body).expect("upload real Linux PTY sender via production RemoteHost");
        assert_eq!(host.read_file(&script, body.len() as u64 + 1).unwrap(), body);
        ShellSpec { program: self.node.clone(), args: vec![script.to_string_lossy().into_owned()], args_are_lyrel_defaults: false }
    }

    pub fn track_pane(&self, pane: u64) { self.panes.lock().unwrap().push(pane); }

    pub fn kill_pane(&self, pane: u64) {
        let (mut stream, _) = self.connect_pane();
        ClientMsg::Kill { pane_id: pane }.encode(&mut stream).unwrap();
        assert!(protocol::DaemonMsg::read(&mut stream).is_err(), "remote Kill must close its command stream");
        self.panes.lock().unwrap().retain(|id| *id != pane);
    }

    pub fn restart_router(&mut self) {
        self.child.kill().expect("kill only isolated local router");
        self.child.wait().unwrap();
        *self.control.lock().unwrap() = None;
        self.child = Self::launch(&self.binary, self.root.path(), self.bundled.as_deref());
        self.await_ready();
    }
}

impl Drop for NativeSshFixture {
    fn drop(&mut self) {
        // Remote daemon belongs to the endpoint fixture, not this test. Only our
        // panes and uniquely named producer directory are removed.
        if let Ok(mut stream) = transport::connect_endpoint_at(&self.endpoint("daemon")) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            if strict_route(&mut stream, &RouteHeader::ssh(self.spec()).for_pane()).is_ok() {
                let _ = protocol::negotiate_pane(&mut stream);
                // Kill requests use a fresh connection per pane below; first
                // stream is only the first one because a command ends it.
                let panes = self.panes.lock().unwrap().clone();
                for (index, pane) in panes.into_iter().enumerate() {
                    if index == 0 { let _ = ClientMsg::Kill { pane_id: pane }.encode(&mut stream); }
                    else if let Ok(mut next) = transport::connect_endpoint_at(&self.endpoint("daemon")) {
                        let _ = next.set_read_timeout(Some(Duration::from_secs(5)));
                        if strict_route(&mut next, &RouteHeader::ssh(self.spec()).for_pane()).is_ok() {
                            let _ = protocol::negotiate_pane(&mut next);
                            let _ = ClientMsg::Kill { pane_id: pane }.encode(&mut next);
                        }
                    }
                }
            }
        }
        if let Some(host) = self.control.lock().unwrap().take() {
            let _ = host.remove(Path::new(&self.remote_dir), true);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
