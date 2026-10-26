use std::io::{BufRead as _, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::{ControlClient, PaneClient};
use lyrel_core::daemon::control::ControlHello;
use lyrel_core::daemon::protocol::{DaemonMsg, PROTOCOL_VERSION, ShellSpec, WinSize};

const DAEMON_ENV: &str = "LYREL_CLI_E2E_DAEMON";
const PASTE_AWARE_FIXTURE_ARG: &str = "--lyrel-e2e-paste-aware-fixture";
const CAPTURE_TAIL_FIXTURE_ARG: &str = "--lyrel-e2e-capture-tail-fixture";
const PASTE_AWARE_TEXT: &str = "lyrel paste aware input";
const PASTE_BURST_WINDOW: Duration = Duration::from_millis(120);
const READY_WITHIN: Duration = Duration::from_secs(30);
const SETTLE_WITHIN: Duration = Duration::from_secs(60);
const CLOSE_WITHIN: Duration = Duration::from_secs(5);

fn main() {
    if std::env::args().any(|arg| arg == CAPTURE_TAIL_FIXTURE_ARG) {
        for n in 1..=6 {
            println!("lyrel_e2e_tail_line_{n}");
        }
        std::io::stdout().flush().expect("flush six fixture lines");
        // Keep the replay observable until the CLI has captured it. Exited
        // panes are reaped even with `run --keep`; no shell prompt is needed.
        let mut done = String::new();
        std::io::stdin()
            .read_line(&mut done)
            .expect("wait for capture completion");
        return;
    }
    if std::env::args().any(|arg| arg == PASTE_AWARE_FIXTURE_ARG) {
        run_paste_aware_fixture();
        return;
    }
    if std::env::var(DAEMON_ENV).as_deref() == Ok("1") {
        if let Err(e) = lyrel_core::daemon::server::run_daemon() {
            eprintln!("e2e daemon exited with error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let tests: &[(&str, fn(&Daemon))] = &[
        (
            "retired_executable_names_still_run_as_ordinary_pty_commands",
            retired_executable_names_still_run_as_ordinary_pty_commands,
        ),
        ("ls_on_an_empty_server", ls_on_an_empty_server),
        (
            "new_builds_a_workspace_with_a_live_pane",
            new_builds_a_workspace_with_a_live_pane,
        ),
        (
            "tab_close_terminates_every_pane_in_the_tab",
            tab_close_terminates_every_pane_in_the_tab,
        ),
        (
            "every_pane_the_cli_files_names_its_workspace_as_owner",
            every_pane_the_cli_files_names_its_workspace_as_owner,
        ),
        (
            "run_streams_output_and_passes_the_exit_code",
            run_streams_output_and_passes_the_exit_code,
        ),
        (
            "run_keep_files_the_pane_so_ls_shows_it",
            run_keep_files_the_pane_so_ls_shows_it,
        ),
        ("send_then_capture_round_trip", send_then_capture_round_trip),
        (
            "parked_shells_keep_cli_identity_input_capture_and_close_scope",
            parked_shells_keep_cli_identity_input_capture_and_close_scope,
        ),
        (
            "send_enter_submits_in_a_paste_aware_raw_mode_tui",
            send_enter_submits_in_a_paste_aware_raw_mode_tui,
        ),
        (
            "status_reports_the_live_server",
            status_reports_the_live_server,
        ),
        (
            "config_dir_alone_resolves_both_endpoints",
            config_dir_alone_resolves_both_endpoints,
        ),
        (
            "legacy_environment_does_not_discover_or_stop_server",
            legacy_environment_does_not_discover_or_stop_server,
        ),
        (
            "events_stream_reports_a_workspace_creation",
            events_stream_reports_a_workspace_creation,
        ),
        (
            "a_reader_that_hung_up_ends_the_pipeline_quietly",
            a_reader_that_hung_up_ends_the_pipeline_quietly,
        ),
        (
            "capture_plain_returns_text_not_escapes",
            capture_plain_returns_text_not_escapes,
        ),
        (
            "capture_still_answers_after_a_resize",
            capture_still_answers_after_a_resize,
        ),
        (
            "capture_tail_trims_a_real_panes_answer",
            capture_tail_trims_a_real_panes_answer,
        ),
        (
            "procs_says_where_the_panes_session_lives",
            procs_says_where_the_panes_session_lives,
        ),
        (
            "exec_returns_the_commands_output_and_exit_code",
            exec_returns_the_commands_output_and_exit_code,
        ),
        (
            "exec_refuses_a_pane_with_no_prompt_marks",
            exec_refuses_a_pane_with_no_prompt_marks,
        ),
        (
            "send_stdin_delivers_bytes_it_never_echoes",
            send_stdin_delivers_bytes_it_never_echoes,
        ),
        (
            "send_paste_asks_the_pane_for_its_bracketed_paste_mode",
            send_paste_asks_the_pane_for_its_bracketed_paste_mode,
        ),
        (
            "worktree_new_sets_up_the_checkout_and_rm_takes_it_down",
            worktree_new_sets_up_the_checkout_and_rm_takes_it_down,
        ),
    ];

    let mut failed = 0;
    for (name, test) in tests {
        let daemon = Daemon::start();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&daemon)));
        drop(daemon);
        match outcome {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                failed += 1;
                println!("test {name} ... FAILED");
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} e2e test(s) failed");
        std::process::exit(1);
    }
}

fn run_paste_aware_fixture() {
    let _raw = raw_mode::enable();
    println!("LYREL_PASTE_AWARE_READY");
    std::io::stdout().flush().expect("flush fixture readiness");

    let mut input = Vec::new();
    let mut last_text_at = None;
    let mut chunk = [0_u8; 256];
    loop {
        let read = std::io::stdin()
            .read(&mut chunk)
            .expect("read fixture input");
        assert_ne!(read, 0, "fixture input ended before Enter");
        let read_at = Instant::now();
        for &byte in &chunk[..read] {
            if byte == b'\r' {
                let submitted = input == PASTE_AWARE_TEXT.as_bytes()
                    && last_text_at.is_some_and(|at| {
                        read_at.saturating_duration_since(at) > PASTE_BURST_WINDOW
                    });
                if submitted {
                    println!("LYREL_PASTE_AWARE_SUBMITTED");
                } else {
                    println!("LYREL_PASTE_AWARE_NOT_SUBMITTED");
                }
                std::io::stdout().flush().expect("flush fixture verdict");
                return;
            }
            input.push(byte);
            last_text_at = Some(read_at);
        }
    }
}

#[cfg(unix)]
mod raw_mode {
    pub struct Guard(libc::termios);

    pub fn enable() -> Guard {
        let mut original = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut original) },
            0,
            "read fixture terminal mode"
        );
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        assert_eq!(
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) },
            0,
            "enable fixture raw mode"
        );
        Guard(original)
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
            }
        }
    }
}

#[cfg(windows)]
mod raw_mode {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };

    pub struct Guard {
        input: HANDLE,
        mode: u32,
    }

    pub fn enable() -> Guard {
        let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut mode = 0;
        assert_ne!(
            unsafe { GetConsoleMode(input, &mut mode) },
            0,
            "read fixture terminal mode"
        );
        let raw = (mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        assert_ne!(
            unsafe { SetConsoleMode(input, raw) },
            0,
            "enable fixture raw mode"
        );
        Guard { input, mode }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                SetConsoleMode(self.input, self.mode);
            }
        }
    }
}

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    config_dir: PathBuf,
    #[cfg(windows)]
    _job: job::Job,
}

impl Daemon {
    fn start() -> Daemon {
        Self::start_with_layout(None)
    }

    fn start_with_layout(relative_config: Option<&str>) -> Daemon {
        let dir = tempfile::TempDir::new().expect("a temp dir for the isolated server");
        let config_dir = relative_config.map_or_else(
            || dir.path().to_path_buf(),
            |relative| dir.path().join(relative),
        );
        std::fs::create_dir_all(&config_dir).expect("create the isolated profile");
        let own = std::env::current_exe().expect("own test binary path");
        let child = Command::new(own)
            .env(DAEMON_ENV, "1")
            .env("LYREL_CONFIG_DIR", &config_dir)
            .env("LYREL_DATA_DIR", &config_dir)
            .env_remove("LYREL_CONTROL_SOCK")
            .env_remove("LYREL_SOCKET")
            // The shell integration's re-entrancy guard. A test run started
            // from inside a lyrel pane would otherwise hand it to every pane
            // this daemon spawns, and each of them would skip its own setup —
            // no prompt marks anywhere, and any assertion about them green for
            // the wrong reason. The injection blanks it per pane too; this is
            // the belt to that's braces, and it also covers the panes the
            // injection declines to touch.
            .env_remove("LYREL_SHELL_INTEGRATION")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the in-test lyrel server");
        #[cfg(windows)]
        let _job = job::Job::kill_on_close(&child);
        let daemon = Daemon {
            child,
            dir,
            config_dir,
            #[cfg(windows)]
            _job,
        };
        daemon.await_ready();
        daemon
    }

    fn control_endpoint(&self) -> PathBuf {
        let file = if cfg!(windows) {
            "control.port"
        } else {
            "control.sock"
        };
        self.config_dir.join(file)
    }

    fn pane_endpoint(&self) -> PathBuf {
        let file = if cfg!(windows) {
            "daemon.port"
        } else {
            "daemon.sock"
        };
        self.config_dir.join(file)
    }

    fn await_ready(&self) {
        let hello = ControlHello::host_rpc("e2e-probe", "e2e-probe");
        let deadline = Instant::now() + READY_WITHIN;
        loop {
            let control_up = ControlClient::connect_at(&self.control_endpoint(), &hello).is_ok();
            let panes_up = PaneClient::at(self.pane_endpoint()).version().is_ok();
            if control_up && panes_up {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the isolated server did not open its endpoints within {READY_WITHIN:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn cli(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lyrel"));
        cmd.args(args)
            .env("LYREL_CONFIG_DIR", &self.config_dir)
            .env("LYREL_DATA_DIR", &self.config_dir)
            .env_remove("LYREL_CONTROL_SOCK")
            .env_remove("LYREL_PANE")
            .env_remove("LYREL_WS")
            .env_remove("LYREL_SOCKET");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cli(args)
            .output()
            .unwrap_or_else(|e| panic!("could not run lyrel {args:?}: {e}"))
    }

    fn run_ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "lyrel {args:?} failed ({}): {}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn run_json(&self, args: &[&str]) -> serde_json::Value {
        let mut with_json: Vec<&str> = args.to_vec();
        with_json.push("--json");
        let out = self.run_ok(&with_json);
        serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("lyrel {args:?} --json printed no JSON ({e}): {out}"))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle as _;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    pub struct Job(HANDLE);

    impl Job {
        pub fn kill_on_close(child: &Child) -> Job {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                assert!(!handle.is_null(), "CreateJobObjectW failed");
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                assert_ne!(
                    SetInformationJobObject(
                        handle,
                        JobObjectExtendedLimitInformation,
                        (&raw const info).cast(),
                        size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    ),
                    0,
                    "SetInformationJobObject failed"
                );
                assert_ne!(
                    AssignProcessToJobObject(handle, child.as_raw_handle()),
                    0,
                    "AssignProcessToJobObject failed"
                );
                Job(handle)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn workdir() -> String {
    std::env::temp_dir().display().to_string()
}

fn one_shot(command: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd.exe".into(), "/d".into(), "/c".into(), command.into()]
    } else {
        vec!["/bin/sh".into(), "-c".into(), command.into()]
    }
}

fn ls_on_an_empty_server(daemon: &Daemon) {
    let out = daemon.run_ok(&["ls"]);
    assert!(out.contains("no workspaces"), "{out}");
}

fn new_builds_a_workspace_with_a_live_pane(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let ws_id = created["id"].as_str().expect("new prints the workspace id");
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    assert!(pane >= 1, "the daemon names panes from 1, got {pane}");

    let listed = daemon.run_json(&["ls"]);
    let workspaces = listed["workspaces"]
        .as_array()
        .expect("ls --json lists workspaces");
    assert_eq!(workspaces.len(), 1, "{listed}");
    assert_eq!(workspaces[0]["id"].as_str(), Some(ws_id), "{listed}");
    assert_eq!(workspaces[0]["panes"].as_u64(), Some(1), "{listed}");

    let panes = daemon.run_ok(&["pane", "ls"]);
    assert!(panes.contains(&format!("%{pane}")), "{panes}");
}

/// `wait --until free` reads freeness off this object, and its whole point is
/// that a process list alone cannot say whether it covers the pane. A real
/// daemon, a real pty: the context has to come back filled, and say this
/// machine holds the pane (#840).
fn procs_says_where_the_panes_session_lives(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");

    let procs = daemon.run_json(&["procs", &format!("%{pane}")]);
    let context = &procs["context"];
    assert!(
        context.is_object(),
        "a current server always answers with a context: {procs}"
    );
    assert_eq!(
        context["local_pty"].as_bool(),
        Some(true),
        "a pane this daemon spawned itself is backed by a pty here: {procs}"
    );
    assert!(
        context.get("remote").is_none(),
        "and it is not the near end of anything: {procs}"
    );
}

fn tab_close_terminates_every_pane_in_the_tab(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let ws_id = created["id"].as_str().expect("new prints the workspace id");
    let tab = daemon.run_json(&["tab", "new", ws_id, "--cwd", &workdir()]);
    let tab_id = tab["tab"].as_str().expect("tab new prints the tab id");
    let first = tab["pane"].as_u64().expect("tab new prints the pane id");
    let first_addr = format!("%{first}");
    let split = daemon.run_json(&["split", &first_addr, "--horizontal"]);
    let second = split["pane"]
        .as_u64()
        .expect("split prints the new pane id");

    let tab_addr = format!("@{tab_id}");
    daemon.run_ok(&["tab", "close", &tab_addr]);

    let deadline = Instant::now() + CLOSE_WITHIN;
    loop {
        let listed = daemon.run_json(&["pane", "ls", "--all"]);
        let running = listed["panes"]
            .as_array()
            .expect("pane ls --all prints the daemon registry");
        let closed_are_gone = running
            .iter()
            .all(|pane| !matches!(pane["pane"].as_u64(), Some(id) if id == first || id == second));
        if closed_are_gone {
            assert_eq!(listed["orphans"].as_u64(), Some(0), "{listed}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "tab close left one of panes %{first} and %{second} live: {listed}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A pane's owner is the workspace allowed to attach to it, and the GUI
/// respawns over anything else — so every way the CLI makes a pane has to
/// stamp that id, not a name of its own. It used to write a literal
/// "lyrel-cli", which left a CLI-built workspace rebuilt from scratch the
/// first time a window opened on it: fresh shells, the live ones orphaned.
fn every_pane_the_cli_files_names_its_workspace_as_owner(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let ws_id = created["id"]
        .as_str()
        .expect("new prints the workspace id")
        .to_string();

    let tab = daemon.run_json(&["tab", "new", &ws_id, "--cwd", &workdir()]);
    let tabbed = tab["pane"].as_u64().expect("tab new prints the pane id");
    daemon.run_json(&["split", &format!("%{tabbed}"), "--horizontal"]);

    let listed = daemon.run_json(&["pane", "ls", "--all"]);
    let panes = listed["panes"]
        .as_array()
        .expect("pane ls --all prints the daemon registry");
    let ours: Vec<&serde_json::Value> = panes
        .iter()
        .filter(|p| p["workspace"].as_str() == Some(ws_id.as_str()))
        .collect();
    // `run --keep` files a pane the same way, but its command has to exit for
    // the CLI to return, and the registry drops the pane with it — so the
    // three that outlive their command are what can be read back here.
    assert_eq!(
        ours.len(),
        3,
        "new, tab new and split each filed one pane: {listed}"
    );
    for pane in ours {
        assert_eq!(
            pane["owner"].as_str(),
            Some(ws_id.as_str()),
            "a pane its workspace holds must name that workspace as owner: {pane}"
        );
    }
}

fn run_streams_output_and_passes_the_exit_code(daemon: &Daemon) {
    let echo = one_shot("echo lyrel_e2e_run_marker");
    let mut args: Vec<&str> = vec!["run", "--"];
    args.extend(echo.iter().map(String::as_str));
    let out = daemon.run(&args);
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lyrel_e2e_run_marker"), "{stdout}");

    let exit = one_shot("exit 7");
    let mut args: Vec<&str> = vec!["run", "--"];
    args.extend(exit.iter().map(String::as_str));
    let out = daemon.run(&args);
    assert_eq!(
        out.status.code(),
        Some(7),
        "the child's exit code must pass through: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_keep_files_the_pane_so_ls_shows_it(daemon: &Daemon) {
    let ws = daemon.run_json(&["ws", "new", "runws"]);
    let ws_id = ws["id"].as_str().expect("ws new prints the id").to_string();

    let mut args: Vec<String> = vec![
        "run".into(),
        "--keep".into(),
        "--ws".into(),
        ws_id.clone(),
        "--".into(),
    ];
    args.extend(one_shot("echo lyrel_e2e_keep_marker"));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = daemon.run(&arg_refs);
    assert!(
        out.status.success(),
        "run --keep failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let listed = daemon.run_json(&["ls"]);
    let workspaces = listed["workspaces"]
        .as_array()
        .expect("ls --json lists workspaces");
    let ours = workspaces
        .iter()
        .find(|w| w["id"].as_str() == Some(ws_id.as_str()))
        .unwrap_or_else(|| panic!("the target workspace is missing from ls: {listed}"));
    assert_eq!(
        ours["panes"].as_u64(),
        Some(1),
        "the kept pane must be filed where every listing sees it: {listed}"
    );

    let panes = daemon.run_json(&["pane", "ls", &ws_id]);
    let filed = panes["panes"]
        .as_array()
        .expect("pane ls --json lists panes");
    assert_eq!(filed.len(), 1, "{panes}");
    assert!(filed[0]["pane"].as_u64().is_some_and(|p| p >= 1), "{panes}");
}

fn config_dir_alone_resolves_both_endpoints(daemon: &Daemon) {
    let out = Command::new(env!("CARGO_BIN_EXE_lyrel"))
        .args(["status", "--json"])
        .env_remove("LYREL_DATA_DIR")
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_PANE")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_SOCKET")
        .env("LYREL_CONFIG_DIR", &daemon.config_dir)
        .output()
        .expect("run lyrel status with only LYREL_CONFIG_DIR");
    assert!(
        out.status.success(),
        "status over LYREL_CONFIG_DIR failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let status: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("status --json prints one JSON object");
    assert_eq!(
        status["pid"].as_u64(),
        Some(u64::from(daemon.child.id())),
        "the answer must come from the daemon LYREL_CONFIG_DIR names: {status}"
    );

    // The control endpoint alone proves nothing: the bug this pins had `status`
    // working while every pane verb reached the wrong socket, because the two
    // endpoints were derived by different rules. Exercise a pane verb over the
    // same lone variable.
    let shell = one_shot(if cfg!(windows) {
        "echo %TERM_PROGRAM%^|%LYREL_CONFIG_DIR% & exit /b 9"
    } else {
        "printf '%s|%s\\n' \"$TERM_PROGRAM\" \"$LYREL_CONFIG_DIR\"; exit 9"
    });
    let mut args = vec!["run", "--"];
    args.extend(shell.iter().map(String::as_str));
    let out = Command::new(env!("CARGO_BIN_EXE_lyrel"))
        .args(args)
        .env_remove("LYREL_DATA_DIR")
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_PANE")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_SOCKET")
        .env("LYREL_CONFIG_DIR", &daemon.config_dir)
        .output()
        .expect("run lyrel run with only LYREL_CONFIG_DIR");
    assert_eq!(
        out.status.code(),
        Some(9),
        "a pane verb must reach the same server the control verb did: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("lyrel|{}", daemon.config_dir.display())),
        "the actual shell must inherit the new product and config identity: {stdout}"
    );
}

// A real daemon stands in for a separately installed other product. Only its
// own environment advertises it; discovering or stopping it is never allowed.
fn legacy_environment_does_not_discover_or_stop_server(daemon: &Daemon) {
    let legacy = Daemon::start_with_layout(Some(if cfg!(windows) {
        "otherterm"
    } else {
        ".config/otherterm"
    }));
    let home = legacy.dir.path();
    let sentinel = legacy.config_dir.join("config.json");
    std::fs::write(&sentinel, b"{\"old_product_fixture\":true}")
        .expect("write legacy data sentinel");
    legacy.run_ok(&["ws", "new", "legacy-workspace"]);
    let before = legacy.run_json(&["ls"]);

    let command = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lyrel"));
        cmd.args(args)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("APPDATA", home)
            .env("OTHERTERM_CONFIG_DIR", &legacy.config_dir)
            .env("OTHERTERM_DATA_DIR", &legacy.config_dir)
            .env("OTHERTERM_CONTROL_SOCK", legacy.control_endpoint())
            .env("OTHERTERM_SOCKET", legacy.pane_endpoint())
            .env_remove("LYREL_CONFIG_DIR")
            .env_remove("LYREL_DATA_DIR")
            .env_remove("LYREL_CONTROL_SOCK")
            .env_remove("LYREL_SOCKET")
            .env_remove("LYREL_PANE")
            .env_remove("LYREL_WS");
        cmd
    };
    let ours = daemon
        .cli(&["status", "--json"])
        .env("OTHERTERM_CONFIG_DIR", &legacy.config_dir)
        .env("OTHERTERM_DATA_DIR", &legacy.config_dir)
        .env("OTHERTERM_CONTROL_SOCK", legacy.control_endpoint())
        .env("OTHERTERM_SOCKET", legacy.pane_endpoint())
        .output()
        .expect("resolve new identity beside the legacy fixture");
    assert!(ours.status.success(), "{ours:?}");
    let ours: serde_json::Value =
        serde_json::from_slice(&ours.stdout).expect("new identity status returns JSON");
    assert_eq!(ours["pid"].as_u64(), Some(u64::from(daemon.child.id())));

    let status = command(&["status", "--json"])
        .output()
        .expect("probe default discovery with only legacy environment");
    assert!(
        !status.status.success(),
        "the old daemon must not answer new-product default discovery: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    let stop = command(&["server", "stop", "--json"])
        .output()
        .expect("stop with only legacy environment");
    assert!(stop.status.success(), "{stop:?}");
    let result: serde_json::Value =
        serde_json::from_slice(&stop.stdout).expect("stop returns JSON");
    assert_eq!(result["stopped"], false, "{result}");
    assert_eq!(legacy.run_json(&["ls"]), before);
    assert_eq!(
        std::fs::read(&sentinel).expect("legacy configuration survives"),
        b"{\"old_product_fixture\":true}"
    );
}

fn send_then_capture_round_trip(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    let address = format!("%{pane}");

    daemon.run_ok(&["send", &address, "echo lyrel_e2e_capture_marker", "--enter"]);

    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let seen = daemon.run_ok(&["capture", &address, "--scrollback"]);
        if seen.contains("lyrel_e2e_capture_marker") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the sent text never showed up in the capture; last capture:\n{seen}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn send_enter_submits_in_a_paste_aware_raw_mode_tui(daemon: &Daemon) {
    let fixture = std::env::current_exe().expect("locate the paste-aware fixture");
    let shell = ShellSpec {
        program: fixture.display().to_string(),
        args: vec![PASTE_AWARE_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("paste-aware-e2e".into()),
            None,
        )
        .expect("spawn the paste-aware raw-mode fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN))
        .expect("bound fixture output reads");
    collect_pane_output_until(&mut pane, b"LYREL_PASTE_AWARE_READY");

    let address = format!("%{}", pane.pane_id());
    daemon.run_ok(&["send", &address, PASTE_AWARE_TEXT, "--enter"]);

    collect_pane_output_until(&mut pane, b"LYREL_PASTE_AWARE_SUBMITTED");
}

fn collect_pane_output_until(session: &mut lyrel_core::client::PaneSession, marker: &[u8]) {
    let mut seen = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend_from_slice(&bytes);
                if seen.windows(marker.len()).any(|window| window == marker) {
                    return;
                }
            }
            Ok(DaemonMsg::Exited { code }) => panic!(
                "pane fixture exited ({code:?}) before {:?}; saw {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(&seen)
            ),
            Ok(_) => {}
            Err(error) => panic!(
                "pane fixture ended before {:?}: {error}; saw {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

fn status_reports_the_live_server(daemon: &Daemon) {
    let status = daemon.run_json(&["status"]);
    assert!(
        status["pid"].as_u64().is_some_and(|pid| pid > 0),
        "{status}"
    );
    assert_eq!(
        status["protocol_version"].as_u64(),
        Some(u64::from(PROTOCOL_VERSION)),
        "{status}"
    );

    let human = daemon.run_ok(&["server", "status"]);
    assert!(human.contains("pid"), "{human}");
}

fn events_stream_reports_a_workspace_creation(daemon: &Daemon) {
    let mut watcher = daemon
        .cli(&["events", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start lyrel events");
    let stdout = watcher.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + SETTLE_WITHIN;
    let mut seen = Vec::new();
    let verdict = 'outer: loop {
        if Instant::now() >= deadline {
            break false;
        }
        daemon.run_ok(&["ws", "new", "evtws"]);
        let round = Instant::now() + Duration::from_secs(5);
        while let Ok(line) = rx.recv_timeout(round.saturating_duration_since(Instant::now())) {
            let is_event = serde_json::from_str::<serde_json::Value>(&line).is_ok();
            seen.push(line);
            if is_event {
                break 'outer true;
            }
        }
    };
    let _ = watcher.kill();
    let _ = watcher.wait();
    assert!(
        verdict,
        "no event line arrived within {SETTLE_WITHIN:?}; saw {seen:?}"
    );
}

/// `lyrel … | head -1` must end the way `cat … | head -1` ends. Rust ignores
/// SIGPIPE and `println!` panics on the resulting error, so without the fix
/// this printed a panic and a backtrace note on a correct invocation.
///
/// Both write paths are covered: `ls` goes through the report emitter, `run`
/// through the loop that streams a child's output. The reader is dropped
/// immediately, long before either has anything to say, so the very first
/// write lands on a pipe with no other end — no need to guess at a buffer size.
fn a_reader_that_hung_up_ends_the_pipeline_quietly(daemon: &Daemon) {
    daemon.run_ok(&["ws", "new", "pipews"]);

    let printer = one_shot("echo lyrel_e2e_pipe_marker");
    let mut streaming: Vec<&str> = vec!["run", "--"];
    streaming.extend(printer.iter().map(String::as_str));

    for args in [vec!["ls"], streaming] {
        let mut child = daemon
            .cli(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("could not spawn lyrel {args:?}: {e}"));
        drop(child.stdout.take().expect("stdout was piped"));

        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("stderr was piped")
            .read_to_string(&mut stderr)
            .expect("reading lyrel's stderr");
        let status = child.wait().expect("waiting for lyrel");

        assert!(
            !stderr.contains("panicked"),
            "lyrel {args:?} panicked when its reader hung up: {stderr}"
        );
        assert!(
            !stderr.to_lowercase().contains("broken pipe"),
            "a hung-up reader is how a pipeline ends, not something to report: \
             lyrel {args:?} said {stderr}"
        );
        // Unix dies of SIGPIPE, so there is no code at all; Windows exits 0.
        // Either way it must not be the failure exit, which is what the error
        // path used to produce.
        assert_ne!(
            status.code(),
            Some(1),
            "lyrel {args:?} treated a hung-up reader as a failure: {stderr}"
        );
    }
}

/// `--plain` against a real pane, end to end through a real daemon.
///
/// The discriminator is the PTY's own line ending: a terminal ends lines with
/// CRLF, so every raw capture carries `\r`, and a rendered one carries none —
/// that CR was an instruction to the grid, not text. It holds whatever the test
/// machine's shell decorates its prompt with, which a check for escape bytes
/// would not: the isolated daemon's shell prints no colour at all.
///
/// What the grid *does* with those bytes (wraps, overwrites, cursor moves) is
/// pinned by the unit tests in `screen.rs`, which can craft the byte stream
/// exactly. This one proves the flag reaches them.
fn capture_plain_returns_text_not_escapes(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    let address = format!("%{pane}");

    daemon.run_ok(&["send", &address, "echo lyrel_e2e_plain_marker", "--enter"]);

    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let raw = daemon.run_ok(&["capture", &address, "--scrollback"]);
        let plain = daemon.run_ok(&["capture", &address, "--scrollback", "--plain"]);
        // The marker renders as soon as the shell echoes the typed command,
        // which can be before the pane has seen a single CR — ConPTY repaints
        // the input line in escape-laden bursts, and `raw` is a separate,
        // slightly earlier snapshot besides. The CR is part of what must
        // settle, not something the marker's arrival already proves.
        if plain.contains("lyrel_e2e_plain_marker") && raw.contains('\r') {
            assert!(
                !plain.contains('\r'),
                "a carriage return is an instruction to the grid, not text:\n{plain:?}"
            );
            assert!(
                !plain.contains('\u{1b}'),
                "an escape survived the grid:\n{plain:?}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the captures never settled (marker rendered, CRLF in the raw \
             bytes); last plain was:\n{plain}\nlast raw was:\n{raw:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A resize with no new application output must not empty `capture` (#841).
/// Use the finite PTY producer so shell prompt repaint and synthesized mode
/// restoration do not turn raw-byte concatenation into a false contract.
fn capture_still_answers_after_a_resize(daemon: &Daemon) {
    let fixture = std::env::current_exe().expect("locate the finite output fixture");
    let shell = ShellSpec {
        program: fixture.display().to_string(),
        args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 100,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("resize-capture-e2e".into()),
            None,
        )
        .expect("spawn the finite output fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN))
        .expect("bound fixture output reads");
    // The trailing CRLF is output too: resizing before it arrives would put
    // a newline-only segment after the six lines this case must capture.
    collect_pane_output_until(&mut pane, b"lyrel_e2e_tail_line_6\r\n");
    let address = format!("%{}", pane.pane_id());
    pane.resize(WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
    })
    .expect("resize the pane");
    loop {
        match pane.recv().expect("wait for the resize acknowledgement") {
            DaemonMsg::Size(size) if size.cols == 80 && size.rows == 24 => break,
            DaemonMsg::Exited { code } => panic!("fixture exited before capture: {code:?}"),
            _ => {}
        }
    }

    let plain = daemon.run_ok(&["capture", &address, "--plain"]);
    let expected: Vec<String> = (1..=6)
        .map(|n| format!("lyrel_e2e_tail_line_{n}"))
        .collect();
    assert_eq!(plain.lines().collect::<Vec<_>>(), expected);

    daemon.run_ok(&["send", &address, "--enter"]);
    loop {
        if let DaemonMsg::Exited { code } = pane.recv().expect("wait for fixture exit") {
            assert_eq!(code, Some(0), "finite output fixture failed");
            break;
        }
    }
    let _ = pane.detach();
}

/// `--tail` consumes a real PTY replay, including its CRLF, without depending
/// on the runner's shell startup files or the height of its prompt. The finite
/// output producer waits for capture to finish before it exits and is reaped.
fn capture_tail_trims_a_real_panes_answer(daemon: &Daemon) {
    let fixture = std::env::current_exe().expect("locate the capture-tail fixture");
    let shell = ShellSpec {
        program: fixture.display().to_string(),
        args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("capture-tail-e2e".into()),
            None,
        )
        .expect("spawn the finite capture-tail fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN))
        .expect("bound fixture output reads");
    collect_pane_output_until(&mut pane, b"lyrel_e2e_tail_line_6\r\n");
    let address = format!("%{}", pane.pane_id());

    let whole = daemon.run_json(&["capture", &address, "--plain", "--scrollback"]);
    let tail = daemon.run_json(&[
        "capture",
        &address,
        "--plain",
        "--scrollback",
        "--tail",
        "2",
    ]);
    let whole_text = whole["text"].as_str().expect("whole capture text");
    let tail_text = tail["text"].as_str().expect("tail capture text");
    assert!(
        whole_text.ends_with(tail_text),
        "a tail has to be the end of the answer it was cut from:\n\
         tail: {tail_text:?}\nwhole: {whole_text:?}"
    );
    assert_eq!(tail_text.lines().count(), 2, "{tail}");
    assert!(tail_text.contains("lyrel_e2e_tail_line_6"), "{tail}");
    assert!(
        whole_text.contains("lyrel_e2e_tail_line_1")
            && !tail_text.contains("lyrel_e2e_tail_line_1"),
        "two lines must drop the first of six: whole {whole}, tail {tail}"
    );
    // The count describes the replay, not the rendered or trimmed text.
    assert_eq!(tail["bytes"], whole["bytes"], "{whole}\n{tail}");
    assert!(
        tail["bytes"]
            .as_u64()
            .is_some_and(|n| n as usize > tail_text.len()),
        "--tail must not shrink the reported replay size: {tail}"
    );

    daemon.run_ok(&["send", &address, "--enter"]);
    loop {
        match pane.recv().expect("wait for capture-tail fixture exit") {
            DaemonMsg::Exited { code } => {
                assert_eq!(code, Some(0), "capture-tail fixture failed");
                break;
            }
            _ => {}
        }
    }
    let _ = pane.detach();
}

/// A pane running `program` with no shell in front of it — so no prompt marks
/// and no line editor, just the pty's own echo. What the tests below type into
/// it comes back exactly as the pane received it.
fn bare_pane(daemon: &Daemon, program: &str) -> u64 {
    let shell = ShellSpec {
        program: program.into(),
        args: Vec::new(),
        args_are_lyrel_defaults: false,
    };
    let session = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("bare-pane-e2e".into()),
            None,
        )
        .unwrap_or_else(|e| panic!("spawn {program}: {e}"));
    let pane = session.pane_id();
    session.detach().expect("leave the bare pane running");
    pane
}

/// Block until the pane's shell has drawn its first prompt: before that there
/// are no marks for `exec` to follow, and it refuses rather than guesses.
fn await_prompt(daemon: &Daemon, address: &str) {
    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let procs = daemon.run_json(&["procs", address]);
        if procs["context"]["at_prompt"].as_bool() == Some(true) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never reported a prompt; last procs: {procs}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn exec_returns_the_commands_output_and_exit_code(daemon: &Daemon) {
    // The marks come from lyrel's integration for zsh/bash/fish/nushell/pwsh;
    // the shell a Windows runner defaults to is not one this can count on.
    if cfg!(windows) {
        return;
    }
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    let address = format!("%{pane}");
    await_prompt(daemon, &address);

    // `sh -c` so the exit code does not depend on which shell the pane runs.
    let out = daemon.run(&[
        "exec",
        &address,
        "--timeout",
        "60",
        "--",
        "sh",
        "-c",
        "'echo lyrel_e2e_exec_$((6*7)); exit 3'",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(3),
        "exec exits with the command's code; stdout {stdout:?}, stderr {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("lyrel_e2e_exec_42"), "{stdout:?}");
    assert!(
        !stdout.contains("6*7") && !stdout.contains("exit 3"),
        "the typed line is not the command's output: {stdout:?}"
    );

    // The pane is back at its prompt, so it takes the next one straight away.
    // `--json` ahead of `--`: after it, it would be an argument to `true`.
    let out = daemon.run_ok(&["exec", &address, "--json", "--timeout", "60", "--", "true"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("--json prints JSON");
    assert_eq!(json["exit"], 0, "{json}");
    assert_eq!(json["exit_code_known"], true, "{json}");
    assert_eq!(json["pane"], pane, "{json}");
    assert_eq!(
        json["output"], "",
        "a command that prints nothing has nothing to show — not the shell's \
         prompt preparation: {json}"
    );
}

fn exec_refuses_a_pane_with_no_prompt_marks(daemon: &Daemon) {
    if cfg!(windows) {
        return;
    }
    let pane = bare_pane(daemon, "/bin/cat");
    let out = daemon.run(&["exec", &format!("%{pane}"), "--", "echo", "never"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("shell-integration"), "{stderr}");
    // Refused before typing: nothing reached the pane.
    let seen = daemon.run_ok(&["capture", &format!("%{pane}"), "--scrollback"]);
    assert!(!seen.contains("never"), "{seen:?}");
}

fn send_via_stdin(daemon: &Daemon, args: &[&str], input: &[u8]) -> Output {
    let mut child = daemon
        .cli(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("could not run lyrel {args:?}: {e}"));
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input)
        .expect("feed lyrel's stdin");
    child.wait_with_output().expect("lyrel ran to completion")
}

fn await_capture(daemon: &Daemon, address: &str, marker: &str) -> String {
    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let seen = daemon.run_ok(&["capture", address, "--scrollback"]);
        if seen.contains(marker) {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "{marker:?} never showed up; last capture:\n{seen}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn send_stdin_delivers_bytes_it_never_echoes(daemon: &Daemon) {
    if cfg!(windows) {
        return;
    }
    let pane = bare_pane(daemon, "/bin/cat");
    let address = format!("%{pane}");
    let payload = "lyrel_e2e_stdin_secret\n";
    let out = send_via_stdin(
        daemon,
        &["send", &address, "--stdin", "--json"],
        payload.as_bytes(),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !stdout.contains(payload.trim_end()),
        "the payload must not come back in the report: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("--json prints JSON");
    assert_eq!(json["source"], "stdin", "{json}");
    assert_eq!(json["bytes"].as_u64(), Some(payload.len() as u64), "{json}");
    await_capture(daemon, &address, payload.trim_end());
}

fn send_paste_asks_the_pane_for_its_bracketed_paste_mode(daemon: &Daemon) {
    if cfg!(windows) {
        return;
    }
    let pane = bare_pane(daemon, "/bin/cat");
    let address = format!("%{pane}");

    // Off to begin with: nothing in the pane has asked for it.
    let json = daemon.run_json(&["send", &address, "lyrel_e2e_one\nlyrel_e2e_two", "--paste"]);
    assert_eq!(json["bracketed_paste_mode"], false, "{json}");
    assert_eq!(json["bracketed"], false, "{json}");
    await_capture(daemon, &address, "lyrel_e2e_two");

    // `cat` hands back what it reads, so the program in the pane is now the
    // one that switched mode 2004 on — the daemon sees it in the output.
    let out = send_via_stdin(daemon, &["send", &address, "--stdin"], b"\x1b[?2004h\n");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let procs = daemon.run_json(&["procs", &address]);
        if procs["context"]["bracketed_paste"].as_bool() == Some(true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pane never reported bracketed paste on: {procs}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let json = daemon.run_json(&["send", &address, "lyrel_e2e_framed", "--paste"]);
    assert_eq!(json["bracketed"], true, "{json}");
    // The frame reached the pty: cat's echo of it carries the opener's tail.
    let seen = await_capture(daemon, &address, "lyrel_e2e_framed");
    assert!(seen.contains("[200~"), "{seen:?}");
}

/// `worktree new` end to end against a real server: the checkout, its
/// `.worktreeinclude` copy, the tab, and `.lyrel/setup` actually running in
/// that tab with its variables; then `ls` sees the pane in it, and `rm` will
/// not take the checkout from under the pane unless told to.
fn worktree_new_sets_up_the_checkout_and_rm_takes_it_down(daemon: &Daemon) {
    // The setup script is a POSIX executable; Windows has no `env` to run it.
    if cfg!(windows) {
        return;
    }
    let repo_dir = tempfile::tempdir().expect("temp repo");
    let repo = repo_dir.path();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join(".gitignore"), ".env\nsetup-ran\n").unwrap();
    std::fs::write(repo.join(".worktreeinclude"), ".env\n").unwrap();
    std::fs::create_dir(repo.join(".lyrel")).unwrap();
    let script = repo.join(".lyrel/setup");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"$LYREL_PORT $LYREL_WORKTREE_NAME\" > setup-ran\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();

    let ws = daemon.run_json(&["new", &workdir()]);
    let ws = ws["id"].as_str().expect("new prints the workspace id");
    let repo_arg = repo.to_str().unwrap();
    let made = daemon.run_json(&["worktree", "new", "e2e-wt", "--ws", ws, "--repo", repo_arg]);
    let path = std::path::PathBuf::from(made["path"].as_str().expect("new prints the path"));
    let pane = made["pane"].as_u64().expect("worktree new prints the pane");
    let port = made["port"].as_u64().expect("worktree new prints the port");
    assert_eq!(made["setup"], true, "{made}");
    assert_eq!(
        std::fs::read_to_string(path.join(".env")).unwrap(),
        "SECRET=1\n"
    );

    let ran = path.join("setup-ran");
    let deadline = Instant::now() + SETTLE_WITHIN;
    let written = loop {
        let text = std::fs::read_to_string(&ran).unwrap_or_default();
        if text.ends_with('\n') {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "setup never ran; the pane shows:\n{}",
            daemon.run_ok(&["capture", &format!("%{pane}"), "--scrollback"])
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(written, format!("{port} e2e-wt\n"));

    let listed = daemon.run_json(&["worktree", "ls", "--repo", repo_arg]);
    let rows = listed["worktrees"].as_array().expect("ls lists worktrees");
    assert_eq!(rows.len(), 2, "{listed}");
    assert_eq!(rows[1]["managed"], true, "{listed}");
    assert_eq!(rows[1]["panes"][0].as_u64(), Some(pane), "{listed}");

    let refused = daemon.run(&["worktree", "rm", "e2e-wt", "--repo", repo_arg]);
    assert!(
        !refused.status.success(),
        "rm took the checkout from under its pane"
    );
    assert!(path.exists());
    let removed = daemon.run_json(&[
        "worktree",
        "rm",
        "e2e-wt",
        "--repo",
        repo_arg,
        "--close-panes",
    ]);
    assert_eq!(removed["snapshot"], "refs/lyrel/trash/e2e-wt", "{removed}");
    assert!(!path.exists());
}
fn retired_executable_names_still_run_as_ordinary_pty_commands(daemon: &Daemon) {
    // An ordinary existing runner fixture, not an agent protocol mock or real agent evidence.
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(ShellSpec {
                program: executable.display().to_string(),
                args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
                args_are_lyrel_defaults: false,
            }),
            Some("ordinary-retired-name-e2e".into()),
            None,
        )
        .expect("the supported-agent list is not an executable allowlist");
    pane.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
    collect_pane_output_until(&mut pane, b"lyrel_e2e_tail_line_6\r\n");
    let address = format!("%{}", pane.pane_id());
    let captured = daemon.run_ok(&["capture", &address, "--scrollback"]);
    assert!(captured.contains("lyrel_e2e_tail_line_6"), "{captured}");
    let procs = daemon.run_json(&["procs", &address]);
    assert!(
        procs["procs"].as_array().unwrap().iter().any(|p| p["name"]
            .as_str()
            .is_some_and(|name| name.contains("claude"))),
        "the ordinary child must still run: {procs}"
    );
    daemon.run_ok(&["send", &address, "finish", "--enter"]);
    loop {
        match pane.recv().expect("the fixture must exit after its input") {
            DaemonMsg::Exited { code } => {
                assert_eq!(code, Some(0));
                break;
            }
            _ => {}
        }
    }
}

fn parked_shells_keep_cli_identity_input_capture_and_close_scope(daemon: &Daemon) {
    use lyrel_core::core::machine::{Axis, PaneSeed};
    use lyrel_core::daemon::control::{ControlRequest, ReplyOk};

    let client = ControlClient::connect_at(
        &daemon.control_endpoint(),
        &ControlHello::host_rpc("parking-cli", "parking-cli"),
    )
    .unwrap();
    let ws = match client
        .request(ControlRequest::WorkspaceCreate {
            name: Some("parked-cli".into()),
            workspace: None,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(workspace) => workspace.id,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    let panes = PaneClient::at(daemon.pane_endpoint());
    let shell = ShellSpec {
        program: if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into(),
        args: if cfg!(windows) {
            vec!["/d".into()]
        } else {
            Vec::new()
        },
        args_are_lyrel_defaults: false,
    };
    let sessions = (0..2)
        .map(|_| {
            panes
                .spawn(
                    None,
                    WinSize {
                        cols: 100,
                        rows: 30,
                        cell_w: 8,
                        cell_h: 16,
                    },
                    Some(shell.clone()),
                    Some(ws.to_string()),
                    None,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (a, b) = (sessions[0].pane_id(), sessions[1].pane_id());
    let seed = |pane| PaneSeed {
        pane,
        cwd: Some(workdir()),
        ssh_spec: None,
        agent: None,
        shell: None,
    };
    client
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(a),
            tab: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Horizontal,
            ratio: 0.5,
            new: seed(b),
            first: false,
        })
        .unwrap();
    let a_addr = format!("%{a}");
    let b_addr = format!("%{b}");
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "set lyrel_park_cli_state=retained"
        } else {
            "lyrel_park_cli_state=retained"
        },
        "--enter",
    ]);
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "echo BEFORE_%lyrel_park_cli_state%"
        } else {
            "printf 'BEFORE_%s\\n' \"$lyrel_park_cli_state\""
        },
        "--enter",
    ]);
    await_capture(daemon, &a_addr, "BEFORE_retained");
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: a,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: true,
            fallback: None,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: None,
        })
        .unwrap();
    let tree = match client
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    assert_eq!(tree.tabs.len(), 1);
    assert!(tree.tabs[0].visible_root().is_none());
    assert!(tree.tabs[0].presentation.pip().is_none());
    let listed = daemon.run_json(&["pane", "ls", "--all"]);
    let registry = listed["panes"].as_array().unwrap();
    assert_eq!(
        registry.len(),
        2,
        "all parked must not create a filler shell: {listed}"
    );
    for pane in [a, b] {
        let entry = registry
            .iter()
            .find(|entry| entry["pane"].as_u64() == Some(pane))
            .unwrap();
        assert_eq!(
            entry["owner"].as_str(),
            Some(ws.to_string().as_str()),
            "{listed}"
        );
        assert_eq!(
            entry["workspace"].as_str(),
            Some(ws.to_string().as_str()),
            "{listed}"
        );
    }
    assert_eq!(
        listed["orphans"].as_u64(),
        Some(0),
        "parked sessions retain ownership: {listed}"
    );
    let workspaces = daemon.run_json(&["ls"]);
    assert_eq!(
        workspaces["workspaces"][0]["panes"].as_u64(),
        Some(2),
        "{workspaces}"
    );
    let tab_list = daemon.run_json(&["tab", "ls", &ws.to_string()]);
    assert_eq!(tab_list["tabs"].as_array().unwrap().len(), 1, "{tab_list}");
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "echo PARKED_%lyrel_park_cli_state%"
        } else {
            "printf 'PARKED_%s\\n' \"$lyrel_park_cli_state\""
        },
        "--enter",
    ]);
    await_capture(daemon, &a_addr, "PARKED_retained");
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: b,
            fallback: None,
        })
        .unwrap();
    let chosen = match client
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    assert_eq!(chosen.tabs[0].visible_root().unwrap().pane_ids(), vec![b]);
    assert!(chosen.tabs[0].presentation.is_parked(a));
    assert!(chosen.tabs[0].presentation.pip().is_none());
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: Some(b),
        })
        .unwrap();
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "echo RESTORED_%lyrel_park_cli_state%"
        } else {
            "printf 'RESTORED_%s\\n' \"$lyrel_park_cli_state\""
        },
        "--enter",
    ]);
    await_capture(daemon, &a_addr, "RESTORED_retained");
    for pane in [a, b] {
        client
            .request(ControlRequest::PanePark {
                workspace: ws,
                pane,
                restore: None,
            })
            .unwrap();
    }
    daemon.run_ok(&["pane", "close", &a_addr]);
    let stale = client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: Some(b),
        })
        .unwrap_err();
    assert_eq!(stale.kind(), std::io::ErrorKind::NotFound);
    let deadline = Instant::now() + CLOSE_WITHIN;
    loop {
        let listed = daemon.run_json(&["pane", "ls", "--all"]);
        let registry = listed["panes"].as_array().unwrap();
        assert!(
            registry
                .iter()
                .any(|entry| entry["pane"].as_u64() == Some(b)),
            "closing one parked pane must retain the other: {listed}"
        );
        if !registry
            .iter()
            .any(|entry| entry["pane"].as_u64() == Some(a))
        {
            assert_eq!(
                registry.len(),
                1,
                "stale restore must not create a session: {listed}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "closed parked pane is still live: {listed}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let after_close = match client
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    assert!(after_close.tabs[0].visible_root().is_none());
    assert!(after_close.tabs[0].presentation.is_parked(b));
    daemon.run_ok(&[
        "send",
        &b_addr,
        if cfg!(windows) {
            "echo SURVIVOR_%LYREL_PANE%"
        } else {
            "printf 'SURVIVOR_%s\\n' \"$LYREL_PANE\""
        },
        "--enter",
    ]);
    await_capture(daemon, &b_addr, &format!("SURVIVOR_{b}"));
    daemon.run_ok(&["pane", "close", &b_addr]);
    drop(sessions);
}
