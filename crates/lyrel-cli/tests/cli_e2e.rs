use std::io::{BufRead as _, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::{ControlClient, PaneClient};
use lyrel_core::daemon::control::ControlHello;
use lyrel_core::daemon::protocol::{DaemonMsg, PROTOCOL_VERSION, ShellSpec, WinSize};

#[path = "cli_e2e/capture.rs"]
mod capture;
#[path = "cli_e2e/execution.rs"]
mod execution;
#[path = "cli_e2e/pane.rs"]
mod pane;
#[path = "cli_e2e/send.rs"]
mod send;
#[path = "cli_e2e/server.rs"]
mod server;
#[path = "cli_e2e/tab.rs"]
mod tab;
#[path = "cli_e2e/workspace.rs"]
mod workspace;
#[path = "cli_e2e/worktree.rs"]
mod worktree;

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
        send::run_paste_aware_fixture();
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
            pane::retired_executable_names_still_run_as_ordinary_pty_commands,
        ),
        ("ls_on_an_empty_server", workspace::ls_on_an_empty_server),
        (
            "new_builds_a_workspace_with_a_live_pane",
            workspace::new_builds_a_workspace_with_a_live_pane,
        ),
        (
            "tab_close_terminates_every_pane_in_the_tab",
            tab::tab_close_terminates_every_pane_in_the_tab,
        ),
        (
            "every_pane_the_cli_files_names_its_workspace_as_owner",
            workspace::every_pane_the_cli_files_names_its_workspace_as_owner,
        ),
        (
            "run_streams_output_and_passes_the_exit_code",
            execution::run_streams_output_and_passes_the_exit_code,
        ),
        (
            "run_keep_files_the_pane_so_ls_shows_it",
            execution::run_keep_files_the_pane_so_ls_shows_it,
        ),
        (
            "send_then_capture_round_trip",
            send::send_then_capture_round_trip,
        ),
        (
            "parked_shells_keep_cli_identity_input_capture_and_close_scope",
            pane::parked_shells_keep_cli_identity_input_capture_and_close_scope,
        ),
        (
            "repeated_pane_pressure_preserves_detached_history_and_closes_only_owned_panes",
            pane::repeated_pane_pressure_preserves_detached_history_and_closes_only_owned_panes,
        ),
        (
            "send_enter_submits_in_a_paste_aware_raw_mode_tui",
            send::send_enter_submits_in_a_paste_aware_raw_mode_tui,
        ),
        (
            "status_reports_the_live_server",
            server::status_reports_the_live_server,
        ),
        (
            "config_dir_alone_resolves_both_endpoints",
            server::config_dir_alone_resolves_both_endpoints,
        ),
        (
            "legacy_environment_does_not_discover_or_stop_server",
            server::legacy_environment_does_not_discover_or_stop_server,
        ),
        (
            "events_stream_reports_a_workspace_creation",
            server::events_stream_reports_a_workspace_creation,
        ),
        (
            "a_reader_that_hung_up_ends_the_pipeline_quietly",
            execution::a_reader_that_hung_up_ends_the_pipeline_quietly,
        ),
        (
            "capture_plain_returns_text_not_escapes",
            capture::capture_plain_returns_text_not_escapes,
        ),
        (
            "capture_still_answers_after_a_resize",
            capture::capture_still_answers_after_a_resize,
        ),
        (
            "capture_tail_trims_a_real_panes_answer",
            capture::capture_tail_trims_a_real_panes_answer,
        ),
        (
            "procs_says_where_the_panes_session_lives",
            pane::procs_says_where_the_panes_session_lives,
        ),
        (
            "exec_returns_the_commands_output_and_exit_code",
            execution::exec_returns_the_commands_output_and_exit_code,
        ),
        (
            "exec_refuses_a_pane_with_no_prompt_marks",
            execution::exec_refuses_a_pane_with_no_prompt_marks,
        ),
        (
            "send_stdin_delivers_bytes_it_never_echoes",
            send::send_stdin_delivers_bytes_it_never_echoes,
        ),
        (
            "send_paste_asks_the_pane_for_its_bracketed_paste_mode",
            send::send_paste_asks_the_pane_for_its_bracketed_paste_mode,
        ),
        (
            "worktree_new_sets_up_the_checkout_and_rm_takes_it_down",
            worktree::worktree_new_sets_up_the_checkout_and_rm_takes_it_down,
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
