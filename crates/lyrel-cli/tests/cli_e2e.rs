use std::collections::VecDeque;
use std::io::{BufRead as _, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lyrel_protocol::client::{ControlClient, PaneClient};
use lyrel_protocol::daemon::control::ControlHello;
use lyrel_protocol::daemon::protocol::{DaemonMsg, PROTOCOL_VERSION, ShellSpec, WinSize};

#[path = "cli_e2e/agents.rs"]
mod agents;
#[path = "cli_e2e/capture.rs"]
mod capture;
#[path = "cli_e2e/capture_exec_workload.rs"]
mod capture_exec_workload;
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
#[path = "cli_e2e/theme.rs"]
mod theme;
#[path = "cli_e2e/titles.rs"]
mod titles;
#[path = "cli_e2e/workspace.rs"]
mod workspace;
#[path = "cli_e2e/worktree.rs"]
mod worktree;

use lyrel_test_support::hex::hex;
use lyrel_test_support::pty::pty_sender;
use lyrel_test_support::synchronized_capture::{
    SYNC_CAPTURE_TEXT, SYNC_NEXT_TEXT, synchronized_capture_frame,
};
use lyrel_test_support::theme_pty::theme_query_sender;

const DAEMON_ENV: &str = "LYREL_CLI_E2E_DAEMON";
const PASTE_AWARE_FIXTURE_ARG: &str = "--lyrel-e2e-paste-aware-fixture";
const CAPTURE_TAIL_FIXTURE_ARG: &str = "--lyrel-e2e-capture-tail-fixture";
const TERMINAL_PTY_FIXTURE_ARG: &str = "--lyrel-e2e-terminal-pty-fixture";
const THEME_PTY_FIXTURE_ARG: &str = "--lyrel-e2e-theme-pty-fixture";
const PASTE_AWARE_TEXT: &str = "lyrel paste aware input";
const PASTE_BURST_WINDOW: Duration = Duration::from_millis(120);
const READY_WITHIN: Duration = Duration::from_secs(30);
const SETTLE_WITHIN: Duration = Duration::from_secs(60);
const CLOSE_WITHIN: Duration = Duration::from_secs(5);

fn main() {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some(capture_exec_workload::WORKLOAD_ARG) => {
            capture_exec_workload::run(args);
            return;
        }
        Some(capture_exec_workload::PRODUCER_ARG) => {
            std::process::exit(capture_exec_workload::produce(args));
        }
        _ => {}
    }
    if std::env::args().any(|arg| arg == agents::FIXTURE_ARG) {
        agents::fixture();
        return;
    }
    if std::env::args().any(|arg| arg == THEME_PTY_FIXTURE_ARG) {
        theme_query_sender();
        return;
    }
    if std::env::args().any(|arg| arg == TERMINAL_PTY_FIXTURE_ARG) {
        pty_sender();
        return;
    }
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
        println!("lyrel_e2e_after_capture");
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

    let options = Options::parse(std::env::args().skip(1));

    let tests: &[Case] = &[
        (
            "agents_report_continuous_state_age",
            agents::agents_report_continuous_state_age,
        ),
        (
            "detached_theme_queries_obey_hint_and_client_ownership",
            theme::detached_theme_queries_obey_hint_and_client_ownership,
        ),
        #[cfg(unix)]
        (
            "detached_theme_queries_survive_public_handoff_refold",
            theme::detached_theme_queries_survive_public_handoff_refold,
        ),
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
            "capture_plain_reads_unfinished_synchronized_output_without_a_gui",
            capture::capture_plain_reads_unfinished_synchronized_output_without_a_gui,
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
        (
            "validated_titles_reach_daemon_facts_from_a_real_pty",
            titles::validated_titles_reach_daemon_facts_from_a_real_pty,
        ),
    ];

    let selected: Vec<Case> = tests
        .iter()
        .copied()
        .filter(|(name, _)| options.matches(name))
        .collect();

    if options.list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        println!();
        println!("{} tests, 0 benchmarks", selected.len());
        return;
    }
    if selected.is_empty() {
        match options.filter.as_deref() {
            Some(filter) => eprintln!("cli_e2e: no tests matched `{filter}`"),
            None => eprintln!("cli_e2e: no tests available on this platform"),
        }
        std::process::exit(2);
    }

    let total = selected.len();
    // One worker per case at most; extra threads would only contend.
    let concurrency = options.concurrency.min(total);
    // The lock is only held while a case is claimed, never while it runs: an
    // e2e case owns a daemon and a temp profile, and unrelated cases must be
    // able to claim and start their own in parallel.
    let queue = Mutex::new(VecDeque::from(selected));
    let failed = AtomicUsize::new(0);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            scope.spawn(|| {
                while let Some((name, test)) = claim(&queue) {
                    let case_started = Instant::now();
                    // Starting the daemon belongs inside the guard: a failed
                    // readiness probe is that case's failure, not a worker
                    // that abandons the rest of the queue.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let daemon = Daemon::start();
                        test(&daemon);
                    }));
                    let elapsed = case_started.elapsed().as_secs_f64();
                    match outcome {
                        Ok(()) => println!("test {name} ... ok ({elapsed:.2}s)"),
                        Err(_) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            println!("test {name} ... FAILED ({elapsed:.2}s)");
                        }
                    }
                }
            });
        }
    });
    let elapsed = started.elapsed().as_secs_f64();
    let failed = failed.load(Ordering::Relaxed);
    if failed > 0 {
        eprintln!("{failed} of {total} e2e test(s) failed ({elapsed:.2}s)");
        std::process::exit(1);
    }
    println!("all {total} e2e tests passed ({elapsed:.2}s)");
}

/// The `harness = false` stand-in for libtest's argument grammar.
///
/// CI runs this target as part of a whole-workspace `cargo test`, which passes
/// `--nocapture`; local runs add a substring filter, `--exact`, `--list`, or a
/// thread override. Anything else is rejected instead of silently becoming a
/// filter that selects nothing.
struct Options {
    filter: Option<String>,
    exact: bool,
    list: bool,
    concurrency: usize,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Options {
        let mut args = args.into_iter();
        let mut filter = None;
        let mut exact = false;
        let mut list = false;
        let mut threads = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--nocapture" => {}
                "--exact" => exact = true,
                "--list" => list = true,
                "--test-threads" => {
                    let value = args
                        .next()
                        .unwrap_or_else(|| usage_error("--test-threads needs a value"));
                    threads = Some(threads_value(&value, "--test-threads"));
                }
                _ if arg.starts_with("--test-threads=") => {
                    threads = Some(threads_value(
                        &arg["--test-threads=".len()..],
                        "--test-threads=",
                    ));
                }
                _ if arg.starts_with('-') => usage_error(&format!("unrecognized option `{arg}`")),
                _ => {
                    if filter.replace(arg).is_some() {
                        usage_error("only one test filter is supported");
                    }
                }
            }
        }
        let concurrency = threads
            .or_else(threads_from_env)
            .unwrap_or_else(default_concurrency);
        Options {
            filter,
            exact,
            list,
            concurrency,
        }
    }

    fn matches(&self, name: &str) -> bool {
        let Some(filter) = self.filter.as_deref() else {
            return true;
        };
        if self.exact {
            name == filter
        } else {
            name.contains(filter)
        }
    }
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 4))
        .unwrap_or(4)
}

/// One registered e2e case: its name and its daemon-bound body.
type Case = (&'static str, fn(&Daemon));

/// Takes the next case. The mutex guard lives only for this call, so a slow
/// case never blocks other workers from claiming their own.
fn claim(queue: &Mutex<VecDeque<Case>>) -> Option<Case> {
    queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop_front()
}

fn threads_from_env() -> Option<usize> {
    let value = std::env::var("LYREL_TEST_THREADS").ok()?;
    Some(threads_value(value.trim(), "LYREL_TEST_THREADS"))
}

fn threads_value(raw: &str, source: &str) -> usize {
    match raw.parse::<usize>() {
        Ok(value) if value > 0 => value,
        _ => usage_error(&format!("{source} must be a positive integer, got `{raw}`")),
    }
}

fn usage_error(message: &str) -> ! {
    eprintln!("cli_e2e: {message}");
    eprintln!("usage: cli_e2e [FILTER] [--exact] [--list] [--nocapture] [--test-threads=N]");
    std::process::exit(2);
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
        let mut command = isolated_command(own, &config_dir);
        command.env(DAEMON_ENV, "1");
        Self::launch(dir, config_dir, command)
    }

    fn launch(dir: tempfile::TempDir, config_dir: PathBuf, mut command: Command) -> Daemon {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the isolated lyrel server");
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
        let mut cmd = isolated_command(env!("CARGO_BIN_EXE_lyrel"), &self.config_dir);
        cmd.args(args);
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

fn isolated_command(program: impl AsRef<std::ffi::OsStr>, config_dir: &std::path::Path) -> Command {
    let mut command = Command::new(program);
    command
        .env("LYREL_CONFIG_DIR", config_dir)
        .env("LYREL_DATA_DIR", config_dir)
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_PANE")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_SOCKET")
        // A harness launched from a Lyrel pane must not cause a fresh shell
        // in the isolated daemon to skip its own integration.
        .env_remove("LYREL_SHELL_INTEGRATION");
    command
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

fn collect_pane_output_until(session: &mut lyrel_protocol::client::PaneSession, marker: &[u8]) {
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
