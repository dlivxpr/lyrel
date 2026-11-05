//! Opt-in real-shell acceptance. Run only inside a private Linux PID namespace.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::{Child, Command, Stdio};

struct ShellRemote {
    home: String,
    shell: String,
}

impl ShellRemote {
    fn new(home: &std::path::Path) -> Self {
        assert_eq!(
            std::env::var("LYREL_INSTALL_PID_NAMESPACE").as_deref(),
            Ok("1"),
            "these tests can stop a daemon: use a private PID namespace"
        );
        assert_eq!(
            std::process::id(),
            1,
            "run the test executable directly under unshare --pid --fork --mount-proc"
        );
        std::fs::create_dir_all(home).unwrap();
        Self {
            home: home.to_str().unwrap().into(),
            shell: std::env::var("LYREL_TEST_INSTALL_SHELL").expect("set the actual login shell"),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.shell);
        if self.shell.ends_with("fish") {
            command.arg("--no-config");
        }
        command
            .env("HOME", &self.home)
            .env("LYREL_CONFIG_DIR", format!("{}/config", self.home));
        command
    }
}

impl RemoteOps for ShellRemote {
    fn home_dir(&self) -> Result<String, String> {
        Ok(self.home.clone())
    }
    fn run(&self, command: &str) -> Result<ExecOutput, String> {
        let out = self
            .command()
            .args(["-c", command])
            .output()
            .map_err(|e| e.to_string())?;
        Ok(ExecOutput {
            status: out.status.code().map(|s| s as u32),
            stdout: String::from_utf8_lossy(&out.stdout).into(),
            stderr: String::from_utf8_lossy(&out.stderr).into(),
        })
    }
    fn spawn_detached(&self, command: &str) -> Result<(), String> {
        let out = self.run(command)?;
        if out.success() {
            Ok(())
        } else {
            Err(out.failure_reason())
        }
    }
    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, String> {
        match std::fs::metadata(path) {
            Ok(m) => Ok(Some(RemoteStat {
                size: m.len(),
                mode: m.mode(),
                is_dir: m.is_dir(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn mkdir(&self, path: &str) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())
    }
    fn chmod(&self, path: &str, mode: u32) -> Result<(), String> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| e.to_string())
    }
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|e| e.to_string())
    }
    fn remove_file(&self, path: &str) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
    fn list_dir(&self, path: &str) -> Result<Option<Vec<String>>, String> {
        match std::fs::read_dir(path) {
            Ok(entries) => entries
                .map(|e| {
                    e.map(|e| e.file_name().to_string_lossy().into_owned())
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn release() -> FakeRelease {
    let bytes = std::fs::read(
        std::env::var("LYREL_TEST_SERVER").expect("set an actual lyrel-server binary"),
    )
    .unwrap();
    FakeRelease {
        asset_bytes: bytes.clone(),
        manifest_of: bytes,
        ..FakeRelease::new()
    }
}

fn real_installer<'a>(
    ops: &'a ShellRemote,
    release: &'a FakeRelease,
    user: &'a FakeUser,
) -> Installer<'a> {
    Installer::new(ops, release, user, "isolated-shell")
        .with_version(VERSION)
        .with_timeouts(Duration::from_secs(3), Duration::from_millis(20))
        .with_shutdown_timeout(Duration::from_secs(3))
}

#[test]
#[ignore = "requires an actual shell/server and a private Linux PID namespace"]
fn real_shell_installs_and_restarts_a_daemon() {
    let temp = tempfile::tempdir().unwrap();
    let ops = ShellRemote::new(&temp.path().join("home"));
    let (release, user) = (release(), FakeUser::approving());
    let installer = real_installer(&ops, &release, &user);
    let report = installer
        .run()
        .expect("actual shell must accept the daemon launch");
    assert!(report.installed && report.launched);
    assert!(
        installer.quick_prove().is_some(),
        "quick probe sees the running daemon"
    );
    installer
        .restart_daemon()
        .expect("actual shell must stop and restart the daemon");
    assert!(
        !installer.run().unwrap().launched,
        "reconnect reuses the restarted daemon"
    );
}

#[test]
#[ignore = "requires an actual shell/server and a private Linux PID namespace"]
fn real_shell_preserves_quoted_and_backslashed_install_paths() {
    let temp = tempfile::tempdir().unwrap();
    let ops = ShellRemote::new(&temp.path().join("home ' and \\\\slashes\\\\withquote\\'"));
    let (release, user) = (release(), FakeUser::approving());
    let installer = real_installer(&ops, &release, &user);
    let report = installer
        .run()
        .expect("quoted paths must survive every shell layer");
    assert!(std::path::Path::new(&report.paths.binary).is_file());
    assert!(installer.quick_prove().is_some());
    installer
        .restart_daemon()
        .expect("nested launch and stop quoting must preserve the path");
}

#[test]
#[ignore = "requires an actual shell/server and a private Linux PID namespace"]
fn real_shell_restarts_a_preexisting_daemon() {
    let temp = tempfile::tempdir().unwrap();
    let ops = ShellRemote::new(&temp.path().join("home"));
    let (release, user) = (release(), FakeUser::approving());
    let installer = real_installer(&ops, &release, &user);
    let paths = installer.paths_for(RemotePlatform::Unix, &ops.home);
    ops.mkdir(&paths.bin_dir).unwrap();
    ops.put(&paths.binary, &release.asset_bytes).unwrap();
    ops.chmod(&paths.binary, 0o755).unwrap();
    let mut daemon = Process(
        Command::new(&paths.binary)
            .arg("--daemon")
            .env("HOME", &ops.home)
            .env("LYREL_CONFIG_DIR", format!("{}/config", ops.home))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ops
        .run(&format!(
            "{} --stdio --bridge </dev/null",
            shell_quote(&paths.binary)
        ))
        .unwrap()
        .success()
    {
        assert!(Instant::now() < deadline, "fixture daemon must serve");
        std::thread::sleep(Duration::from_millis(10));
    }
    installer
        .restart_daemon()
        .expect("the login shell must hand the stop script to POSIX sh");
    assert!(
        daemon.0.try_wait().unwrap().is_some(),
        "restart must stop the original daemon"
    );
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires an actual shell/server and a private Linux PID namespace"]
fn real_process_discovery_and_restart_leave_a_bridge_alive() {
    let temp = tempfile::tempdir().unwrap();
    let ops = ShellRemote::new(&temp.path().join("home"));
    let (release, user) = (release(), FakeUser::approving());
    let installer = real_installer(&ops, &release, &user);
    let paths = installer.paths_for(RemotePlatform::Unix, &ops.home);
    ops.mkdir(&paths.bin_dir).unwrap();
    let old = format!("{}/lyrel-server-c0p0", paths.bin_dir);
    let bridge = format!("{}/lyrel-server-pane-bridge", paths.bin_dir);
    for path in [&old, &bridge, &paths.binary] {
        ops.put(path, &release.asset_bytes).unwrap();
        ops.chmod(path, 0o755).unwrap();
    }
    // Give the daemon a PID ending in 9. The bridge at the next power of ten
    // then sorts first in /proc's lexical glob, independently of scheduler timing.
    let mut last = Command::new("true").spawn().unwrap();
    loop {
        let id = last.id();
        last.wait().unwrap();
        if id.to_string().chars().all(|c| c == '8') {
            break;
        }
        assert!(id < 10_000, "fixture PID padding remained bounded");
        last = Command::new("true").spawn().unwrap();
    }
    let mut daemon = Process(
        Command::new(&old)
            .arg("--daemon")
            .env("HOME", &ops.home)
            .env("LYREL_CONFIG_DIR", format!("{}/config", ops.home))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ops
        .run(&format!(
            "{} --stdio --bridge </dev/null",
            shell_quote(&paths.binary)
        ))
        .unwrap()
        .success()
    {
        assert!(Instant::now() < deadline, "fixture daemon must serve");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut bridge = Process(
        Command::new(&bridge)
            .arg("--stdio")
            .env("HOME", &ops.home)
            .env("LYREL_CONFIG_DIR", format!("{}/config", ops.home))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(
        bridge.0.id().to_string() < daemon.0.id().to_string(),
        "bridge must precede daemon lexically"
    );
    let adopted = installer.run().unwrap();
    assert!(!adopted.launched);
    // The installed current-dialect image is usable. Remove it so the public
    // install journey must discover and adopt the compatible daemon's image.
    ops.remove_file(&paths.binary).unwrap();
    let adopted = installer.run().unwrap();
    assert_eq!(
        adopted.paths.binary, old,
        "adoption must identify the daemon, not the earlier bridge"
    );
    ops.put(&paths.binary, &release.asset_bytes).unwrap();
    ops.chmod(&paths.binary, 0o755).unwrap();
    installer
        .restart_daemon()
        .expect("restart must target the daemon");
    use std::os::unix::process::ExitStatusExt as _;
    if let Some(status) = bridge.0.try_wait().unwrap() {
        assert_ne!(
            status.signal(),
            Some(15),
            "the bridge may exit when its daemon closes, but must not receive the restart's SIGTERM"
        );
    }
    assert!(
        daemon.0.try_wait().unwrap().is_some(),
        "old daemon must have stopped"
    );
}
