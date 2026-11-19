//! The lightweight graphics-fixture process: locating, verifying, on-demand
//! building and driving the `lyrel-graphics-fixture` binary that the desktop
//! terminal consumer tests spawn for a real daemon and real PTY producers.
//!
//! GUI-free on purpose: everything here speaks the real daemon protocol and
//! spawns real processes, so it never touches GPUI and stays usable from any
//! test harness. The terminal-side view-driving helpers live in
//! `lyrel-terminal`'s `test_support` module and build on this process
//! plumbing.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use lyrel_core::client::PaneClient;
use lyrel_core::daemon::protocol::{
    ClientMsg, DaemonMsg, ShellSpec, WinSize, negotiate_pane_rendering,
};
use lyrel_core::daemon::transport::{Stream, connect_endpoint_at};

const GRAPHICS_FIXTURE_BIN: &str = "lyrel-graphics-fixture";
const GRAPHICS_FIXTURE_BIN_ENV: &str = "LYREL_GRAPHICS_FIXTURE_BIN";

/// The workspace root that holds `crates/lyrel-test-support/Cargo.toml`.
/// `CARGO_MANIFEST_DIR` is this crate's own directory, so the workspace is
/// exactly two levels up; the current-directory fallback keeps
/// manually-launched test binaries working when they run from the checkout.
fn workspace_root() -> PathBuf {
    let compiled = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = compiled
        .parent()
        .and_then(|parent| parent.parent())
        .map(Path::to_path_buf);
    if let Some(workspace) =
        workspace.filter(|root| root.join("crates/lyrel-test-support/Cargo.toml").is_file())
    {
        return workspace;
    }
    let current = std::env::current_dir().expect("test process working directory");
    if current
        .join("crates/lyrel-test-support/Cargo.toml")
        .is_file()
    {
        return current;
    }
    panic!(
        "the graphics fixture sources are not reachable from {} or {}; run the tests from a \
         lyrel source checkout",
        compiled.display(),
        current.display()
    );
}

static GRAPHICS_FIXTURE_EXPECTED_IDENTITY: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        let root = workspace_root();
        crate::fixture_identity::fixture_source_identity(&root).unwrap_or_else(|error| {
            panic!(
                "cannot identify the graphics fixture sources under {}: {error}",
                root.display()
            )
        })
    });

fn graphics_fixture_reported_identity(path: &Path) -> Option<String> {
    let output = Command::new(path)
        .arg("identity")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn graphics_fixture_verify(path: &Path) {
    let expected = GRAPHICS_FIXTURE_EXPECTED_IDENTITY.as_str();
    match graphics_fixture_reported_identity(path) {
        Some(reported) if reported == expected => {}
        Some(reported) => panic!(
            "graphics fixture binary {} was built from other sources (expected identity {expected}, \
             reported {reported}); rebuild it with `cargo build --locked -p lyrel-test-support \
             --bin lyrel-graphics-fixture`",
            path.display()
        ),
        None => panic!(
            "graphics fixture binary {} did not report its build identity; rebuild it with \
             `cargo build --locked -p lyrel-test-support --bin lyrel-graphics-fixture`",
            path.display()
        ),
    }
}

/// The fixture binary Cargo would place beside this test executable's Cargo
/// profile: `<profile-dir>/<bin><exe-suffix>`.
fn graphics_fixture_default_path() -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    // Cargo 1.100 puts libtest under `build/<package>/<hash>/out` rather than
    // `deps/`, and a default per-worktree target dir no longer carries a
    // `.cargo-lock` marker in the profile directory — so locate the profile
    // by the directories Cargo always creates there (`deps`, `build`,
    // `incremental`) instead of counting parents or trusting the lock file.
    // An explicitly supplied fixture also supports relocated test artifacts.
    let profile = exe
        .ancestors()
        .skip(1)
        .find(|directory| {
            directory.join(".cargo-lock").is_file()
                || directory.join("deps").is_dir()
                || directory.join("build").is_dir()
                || directory.join("incremental").is_dir()
        })
        .unwrap_or_else(|| {
            panic!(
                "cannot locate the Cargo profile for {}; set {GRAPHICS_FIXTURE_BIN_ENV} \
                 to a matching prebuilt {GRAPHICS_FIXTURE_BIN}",
                exe.display()
            )
        });
    profile.join(format!(
        "{GRAPHICS_FIXTURE_BIN}{}",
        std::env::consts::EXE_SUFFIX
    ))
}

/// The host triple, used only to decide whether the test executable sits under
/// a `target/<triple>/<profile>` directory. Tests cannot execute for another
/// target, so this is the exact directory `cargo test --target` used.
fn graphics_fixture_host_triple() -> Option<&'static str> {
    static HOST: std::sync::LazyLock<Option<String>> = std::sync::LazyLock::new(|| {
        let output = std::process::Command::new("rustc")
            .arg("-vV")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
    });
    HOST.as_deref()
}

/// Build the fixture once, under the same Cargo profile/target/target-dir the
/// test binary came from, and return the executable Cargo reported.
fn graphics_fixture_run_cargo(profile_dir: &Path) -> PathBuf {
    let root = workspace_root();
    let manifest = root.join("crates/lyrel-test-support/Cargo.toml");
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(&root)
        .arg("build")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--bin")
        .arg(GRAPHICS_FIXTURE_BIN)
        .arg("--message-format=json-render-diagnostics");

    let profile = profile_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("debug");
    if profile == "release" {
        command.arg("--release");
    } else if profile != "debug" {
        command.args(["--profile", profile]);
    }

    // A `--target <triple>` invocation nests artifacts under
    // `target/<triple>/<profile>`; reuse that directory instead of compiling
    // the same dependency graph a second time under `target/<profile>`.
    let triple = profile_dir.parent().and_then(|dir| {
        let name = dir.file_name()?.to_str()?;
        (Some(name) == graphics_fixture_host_triple()).then(|| name.to_owned())
    });
    let target_root = match &triple {
        Some(_) => profile_dir
            .parent()
            .and_then(|dir| dir.parent())
            .unwrap_or(profile_dir),
        None => profile_dir.parent().unwrap_or(profile_dir),
    };
    command.arg("--target-dir").arg(target_root);
    if let Some(triple) = &triple {
        command.args(["--target", triple.as_str()]);
    }

    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cannot run {}: {error}", env!("CARGO")));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "building {GRAPHICS_FIXTURE_BIN} for the graphics consumer tests failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        if message["target"]["name"] != GRAPHICS_FIXTURE_BIN {
            continue;
        }
        if let Some(executable) = message["executable"].as_str() {
            let executable = PathBuf::from(executable);
            return if executable.is_absolute() {
                executable
            } else {
                root.join(executable)
            };
        }
    }
    panic!(
        "cargo built {GRAPHICS_FIXTURE_BIN} but reported no executable\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

fn graphics_fixture_build() -> PathBuf {
    let profile_dir = graphics_fixture_default_path()
        .parent()
        .expect("fixture executable directory")
        .to_path_buf();
    std::fs::create_dir_all(&profile_dir)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", profile_dir.display()));
    let lock_path = profile_dir.join(".lyrel-graphics-fixture-build.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", lock_path.display()));
    use fs2::FileExt as _;
    lock.lock_exclusive()
        .unwrap_or_else(|error| panic!("cannot lock {}: {error}", lock_path.display()));
    // Parallel test processes may all have missed the binary; whoever wins
    // the lock builds it, the rest reuse that result.
    let default = graphics_fixture_default_path();
    if default.is_file()
        && graphics_fixture_reported_identity(&default).as_deref()
            == Some(GRAPHICS_FIXTURE_EXPECTED_IDENTITY.as_str())
    {
        return default;
    }
    graphics_fixture_run_cargo(&profile_dir)
}

/// The verified lightweight fixture binary. Never falls back to the GUI test
/// binary or a fixture without a matching build identity.
pub fn bin() -> &'static Path {
    static BIN: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        let path = if let Some(configured) = std::env::var_os(GRAPHICS_FIXTURE_BIN_ENV) {
            let path = PathBuf::from(configured);
            graphics_fixture_verify(&path);
            path
        } else {
            let default = graphics_fixture_default_path();
            if default.is_file()
                && graphics_fixture_reported_identity(&default).as_deref()
                    == Some(GRAPHICS_FIXTURE_EXPECTED_IDENTITY.as_str())
            {
                default
            } else {
                let built = graphics_fixture_build();
                graphics_fixture_verify(&built);
                built
            }
        };
        graphics_fixture_stage_conpty(&path);
        path
    });
    &BIN
}

/// A `ShellSpec` that runs the verified fixture in `mode` — one of the
/// producer modes the fixture binary dispatches (`kitty`, `theme`, `mouse`,
/// `mouse-utf8`, `glyph`).
pub fn graphics_fixture_shell(mode: &str) -> ShellSpec {
    ShellSpec {
        program: bin().to_string_lossy().into_owned(),
        args: vec![mode.to_owned()],
        args_are_lyrel_defaults: false,
    }
}

/// `portable-pty` loads `conpty.dll` from the daemon executable's directory and
/// the daemon verifies the pinned triple before every pane. The test entry
/// stages that triple beside the test executable, which lives in a different
/// directory than the fixture binary, so carry the verified bytes across.
#[cfg(windows)]
fn graphics_fixture_stage_conpty(fixture: &Path) {
    let source_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .expect("test executable directory");
    let target_dir = fixture.parent().expect("fixture executable directory");
    if source_dir.as_path() == target_dir {
        return;
    }
    const FILES: [&str; 3] = ["conpty.dll", "OpenConsole.exe", "LICENSE-ConPTY.txt"];
    let needs_copy = FILES.iter().any(|name| {
        let source = source_dir.join(name);
        let target = target_dir.join(name);
        match (source.metadata(), target.metadata()) {
            (Ok(source), Ok(target)) => source.len() != target.len(),
            (Ok(_), Err(_)) => true,
            // A pre-staged fixture is good enough; the daemon verifies the
            // pinned SHA-256 set before any pane starts.
            (Err(_), Ok(_)) => false,
            (Err(_), Err(_)) => true,
        }
    });
    if !needs_copy {
        return;
    }
    let lock_path = target_dir.join(".lyrel-graphics-fixture-conpty.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", lock_path.display()));
    {
        use fs2::FileExt as _;
        lock.lock_exclusive()
            .unwrap_or_else(|error| panic!("cannot lock {}: {error}", lock_path.display()));
        for name in FILES {
            let source = source_dir.join(name);
            let target = target_dir.join(name);
            let source_bytes = match std::fs::read(&source) {
                Ok(bytes) => bytes,
                Err(error) => {
                    assert!(
                        target.is_file(),
                        "the pinned ConPTY set is not staged beside the test executable ({}) or \
                         the fixture ({}): {error}; run the Windows test entry so conpty.dll, \
                         OpenConsole.exe and LICENSE-ConPTY.txt are staged",
                        source.display(),
                        target.display()
                    );
                    continue;
                }
            };
            if std::fs::read(&target).ok().as_deref() == Some(source_bytes.as_slice()) {
                continue;
            }
            let staging = target.with_extension(format!("staged-{}", std::process::id()));
            std::fs::write(&staging, &source_bytes)
                .unwrap_or_else(|error| panic!("cannot stage {}: {error}", staging.display()));
            if target.exists() {
                std::fs::remove_file(&target)
                    .unwrap_or_else(|error| panic!("cannot replace {}: {error}", target.display()));
            }
            std::fs::rename(&staging, &target)
                .unwrap_or_else(|error| panic!("cannot install {}: {error}", target.display()));
        }
    }
}

#[cfg(not(windows))]
fn graphics_fixture_stage_conpty(_fixture: &Path) {}

/// A real fixture daemon in an isolated config/data directory, speaking the
/// production protocol on both its control and pane endpoints. Desktop
/// consumer tests spawn one process for the whole suite file instead of
/// re-launching the multi-hundred-megabyte GUI test binary per daemon.
pub struct GraphicsDaemonProcess {
    child: Child,
    /// The isolated config/data directory; public for tests that need a cwd
    /// or HOME inside the daemon's sandbox.
    pub dir: tempfile::TempDir,
}

impl GraphicsDaemonProcess {
    pub fn start() -> Self {
        Self::start_with(|_, _| {})
    }

    pub fn start_with(configure: impl FnOnce(&mut Command, &Path)) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let mut command = Command::new(bin());
        command
            .arg("daemon")
            .env("LYREL_GRAPHICS_TEST_DAEMON", dir.path())
            .env("LYREL_CONFIG_DIR", dir.path())
            .env("LYREL_DATA_DIR", dir.path())
            .env("LYREL_CONTROL_SOCK", dir.path().join("control.sock"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure(&mut command, dir.path());
        let child = command.spawn().unwrap();
        let daemon = Self { child, dir };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let ready = (|| -> std::io::Result<()> {
                use lyrel_core::daemon::control::{
                    ControlClientMsg, ControlHello, ControlServerMsg,
                };

                // Bound the actual protocol reads/writes too: the outer startup
                // deadline alone cannot interrupt a silent listening handler.
                for endpoint in ["control", "daemon"] {
                    let mut stream = connect_endpoint_at(&daemon.endpoint(endpoint))?;
                    let wait = deadline
                        .checked_duration_since(std::time::Instant::now())
                        .filter(|wait| !wait.is_zero())
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "graphics daemon startup deadline expired",
                            )
                        })?
                        .min(std::time::Duration::from_millis(250));
                    stream.set_read_timeout(Some(wait))?;
                    stream.set_write_timeout(Some(wait))?;
                    if endpoint == "control" {
                        let hello = ControlHello::host_rpc("graphics-gpui", "graphics-gpui");
                        ControlClientMsg::Hello(hello.clone()).encode(&mut stream)?;
                        match ControlServerMsg::read(&mut stream)? {
                            ControlServerMsg::HelloOk(ok)
                                if ok.control_version == hello.control_version => {}
                            other => {
                                return Err(std::io::Error::other(format!(
                                    "unexpected control readiness reply: {other:?}"
                                )));
                            }
                        }
                    } else {
                        ClientMsg::Version.encode(&mut stream)?;
                        if !matches!(DaemonMsg::read(&mut stream)?, DaemonMsg::Version(_)) {
                            return Err(std::io::Error::other("unexpected pane readiness reply"));
                        }
                    }
                }
                Ok(())
            })();
            if ready.is_ok() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "isolated production daemon did not become ready: {ready:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        daemon
    }

    pub fn endpoint(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!(
            "{name}.{}",
            if cfg!(windows) { "port" } else { "sock" }
        ))
    }

    pub fn panes(&self) -> PaneClient {
        PaneClient::at(self.endpoint("daemon"))
    }

    pub fn spawn(&self) -> u64 {
        self.spawn_with_cwd(None)
    }

    pub fn spawn_with_cwd(&self, cwd: Option<PathBuf>) -> u64 {
        let shell = graphics_fixture_shell("kitty");
        let mut session = self
            .panes()
            .spawn(
                cwd,
                WinSize {
                    cols: 80,
                    rows: 24,
                    cell_w: 10,
                    cell_h: 20,
                },
                Some(shell),
                None,
                None,
            )
            .unwrap();
        session
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let mut text = Vec::new();
        while !text
            .windows(b"KITTY_READY".len())
            .any(|part| part == b"KITTY_READY")
        {
            // The child may print before its first subscriber attaches.
            match session.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => text.extend(bytes),
                DaemonMsg::Error(error) => panic!("graphics sender failed: {error}"),
                DaemonMsg::Exited { code } => {
                    panic!("graphics sender exited before KITTY_READY: {code:?}")
                }
                _ => {}
            }
        }
        let id = session.pane_id();
        session.detach().unwrap();
        id
    }

    pub fn stream(&self, pane: u64) -> Stream {
        self.stream_with_capacity(pane, 64 << 20, 128 << 20)
    }

    pub fn stream_with_capacity(
        &self,
        pane: u64,
        encoded_bytes: u64,
        decoded_bytes: u64,
    ) -> Stream {
        self.stream_with_capacity_at(
            pane,
            encoded_bytes,
            decoded_bytes,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
        )
    }

    pub fn stream_with_capacity_at(
        &self,
        pane: u64,
        encoded_bytes: u64,
        decoded_bytes: u64,
        size: WinSize,
    ) -> Stream {
        let mut stream = connect_endpoint_at(&self.endpoint("daemon")).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        // Every consumer attached through this helper is a real production
        // TerminalView: it keeps the monochrome contract, so it declares the
        // render opt-in — the credential `q` routing and `s` answers bind to.
        negotiate_pane_rendering(&mut stream, true).unwrap();
        ClientMsg::GraphicsCapacity {
            encoded_bytes,
            decoded_bytes,
            offers: true,
        }
        .encode(&mut stream)
        .unwrap();
        stream.set_read_timeout(None).unwrap();
        ClientMsg::Attach {
            pane_id: pane,
            size,
            allow_remote_clipboard_write: false,
        }
        .encode(&mut stream)
        .unwrap();
        stream
    }

    /// Spawn a fixture pane that records the mouse reports dispatched to it,
    /// returning the pane id and the recorder's directory. The pane is a real
    /// raw-mode PTY child; asserted reports are read from its stdin file.
    pub fn spawn_mouse_recorder(&self, name: &str) -> (u64, PathBuf) {
        let dir = self.dir.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        let shell = graphics_fixture_shell("mouse");
        let mut session = self
            .panes()
            .spawn(
                Some(dir.clone()),
                WinSize {
                    cols: 80,
                    rows: 24,
                    cell_w: 10,
                    cell_h: 20,
                },
                Some(shell),
                None,
                None,
            )
            .unwrap();
        session
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let mut text = Vec::new();
        while !text
            .windows(b"MOUSE_READY".len())
            .any(|part| part == b"MOUSE_READY")
        {
            match session.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => text.extend(bytes),
                DaemonMsg::Error(error) => panic!("mouse PTY fixture failed: {error}"),
                DaemonMsg::Exited { code } => panic!("mouse PTY fixture exited: {code:?}"),
                _ => {}
            }
        }
        let pane = session.pane_id();
        session.detach().unwrap();
        (pane, dir)
    }

    /// The UTF-8 (mode 1005) mouse recorder on a deliberately wide pane:
    /// clicks past column 222 are what that encoding exists to carry.
    pub fn spawn_utf8_mouse_recorder(&self, name: &str) -> (u64, PathBuf) {
        let dir = self.dir.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        let shell = graphics_fixture_shell("mouse-utf8");
        let mut session = self
            .panes()
            .spawn(
                Some(dir.clone()),
                WinSize {
                    cols: 300,
                    rows: 24,
                    cell_w: 10,
                    cell_h: 20,
                },
                Some(shell),
                None,
                None,
            )
            .unwrap();
        session
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let mut text = Vec::new();
        while !text
            .windows(b"UTF8_MOUSE_READY".len())
            .any(|part| part == b"UTF8_MOUSE_READY")
        {
            match session.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => text.extend(bytes),
                DaemonMsg::Error(error) => panic!("UTF-8 mouse PTY fixture failed: {error}"),
                DaemonMsg::Exited { code } => {
                    panic!("UTF-8 mouse PTY fixture exited: {code:?}")
                }
                _ => {}
            }
        }
        let pane = session.pane_id();
        session.detach().unwrap();
        (pane, dir)
    }
}

impl Drop for GraphicsDaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
