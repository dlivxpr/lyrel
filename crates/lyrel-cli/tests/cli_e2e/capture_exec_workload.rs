use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use lyrel_protocol::client::{ControlClient, PaneClient, PaneSession};
use lyrel_protocol::daemon::control::{ControlHello, ControlRequest, ReplyOk};
use lyrel_protocol::daemon::protocol::{DaemonMsg, WinSize};
use serde_json::{Value, json};

use super::{Daemon, isolated_command};

pub(super) const WORKLOAD_ARG: &str = "--capture-exec-workload";
pub(super) const PRODUCER_ARG: &str = "--lyrel-e2e-capture-exec-producer";
const INITIAL_SIZE: WinSize = WinSize {
    cols: 120,
    rows: 30,
    cell_w: 8,
    cell_h: 16,
};
const LIMIT: Duration = Duration::from_secs(10);
const SEED_ROWS: usize = 2400;
const BUSY_ROWS: usize = 96;
const RESULT_ROWS: usize = 8;
const BUSY_INTERVAL: Duration = Duration::from_millis(32);
#[cfg(windows)]
const FIXTURE: &str = "lyrel-perf-fixture.exe";
#[cfg(not(windows))]
const FIXTURE: &str = "lyrel-perf-fixture";

/// 在预留机器的专用测试账户运行。同一轮使用同一版本的 Release 产物及干净的 Bash。
/// 每次启动独立配置目录、daemon 和窗格，输出一轮 JSONL。
#[derive(Parser)]
#[command(name = "cli_e2e --capture-exec-workload")]
struct Options {
    revision: String,
    cli: PathBuf,
    server: PathBuf,
    bash: PathBuf,
    /// 启动同一版本的 lyrel-app 并附着到测量窗格。Linux headless 不传。
    #[arg(long)]
    gui: Option<PathBuf>,
    /// 确认最终画面时保留窗口五秒，位于计时区间之外。
    #[arg(long, requires = "gui")]
    inspect: bool,
}

pub(super) fn run(args: impl Iterator<Item = OsString>) {
    let options = Options::parse_from(std::iter::once(OsString::from(WORKLOAD_ARG)).chain(args));
    let cli = executable(&options.cli);
    let server = executable(&options.server);
    let bash = executable(&options.bash);
    let gui = options.gui.as_deref().map(executable);
    assert!(
        matches!(
            bash.file_name().and_then(|s| s.to_str()),
            Some("bash" | "bash.exe")
        ),
        "use Bash (Git Bash on Windows) so lyrel loads its shell integration"
    );

    let dir = tempfile::TempDir::new().expect("create isolated workload profile");
    let config = dir.path().to_path_buf();
    let work = config.join("work");
    fs::create_dir(&work).expect("create the workload cwd");
    fs::copy(std::env::current_exe().unwrap(), work.join(FIXTURE)).expect("install PTY producer");
    fs::write(
        config.join("config.json"),
        json!({
            "shell": { "program": bash },
            "install_cli_on_path": false,
            "check_for_updates": false,
            "auto_download_updates": false,
            "restore_session": true
        })
        .to_string(),
    )
    .expect("write isolated config");
    let mut server_command = isolated_command(&server, &config);
    server_command
        .arg("--daemon")
        .arg("--config-dir")
        .arg(&config);
    let daemon = Daemon::launch(dir, config, server_command);
    let mut workload = Workload {
        cli,
        daemon,
        work,
        address: String::new(),
    };
    let status = workload.checked(&["status", "--json"]);
    assert_eq!(
        status["pid"].as_u64(),
        Some(u64::from(workload.daemon.child.id()))
    );
    let cwd = workload.work.to_str().expect("UTF-8 workload path");
    let created = workload.checked(&["new", cwd, "--json"]);
    let pane = created["pane"].as_u64().expect("created pane ID");
    workload.address = format!("%{pane}");
    let mut gui = gui.map(|path| workload.attach_gui(&path, &created, pane));
    let grid = gui.as_ref().map_or(INITIAL_SIZE, |(_, size)| *size);
    emit(json!({
        "kind": "workload", "issue": 436, "revision": options.revision,
        "platform": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "daemon_pid": status["pid"], "server_status": status, "pane": pane,
        "gui_pid": gui.as_ref().map(|(child, _)| child.0.id()), "grid": grid,
        "cli": workload.cli, "server": server, "seed_rows": SEED_ROWS,
        "busy_rows": BUSY_ROWS, "busy_interval_ms": BUSY_INTERVAL.as_millis(),
        "result_rows": RESULT_ROWS, "timeout_ms": LIMIT.as_millis(),
    }));

    workload.await_free();
    workload.checked(&[
        "exec",
        &workload.address,
        "--json",
        "--timeout",
        "8",
        "--",
        &producer_line("S"),
    ]);
    // Let Bash finish redrawing the prompt after its command-end report.
    thread::sleep(Duration::from_millis(100));
    workload.capture("capture_idle", false);
    workload.exec("exec_idle", "E", 7);
    thread::sleep(Duration::from_millis(100));

    workload.send("B");
    let busy_started = wait_for_stamp(&workload.work.join("B.start"));
    workload.await_busy_output(pane);
    emit(
        json!({ "kind": "busy_producer", "pid": busy_started.1, "start_unix_ns": busy_started.0 }),
    );
    let busy_rows = workload.capture("capture_busy", true);
    let refused = workload.invoke(&[
        "exec",
        &workload.address,
        "--timeout",
        "8",
        "--",
        &producer_line("R"),
    ]);
    emit(refused.record("exec_busy", json!({ "exit": refused.output.status.code() })));
    assert_eq!(
        refused.output.status.code(),
        Some(1),
        "{}",
        stderr(&refused.output)
    );
    assert!(
        stderr(&refused.output).contains("not at a shell prompt"),
        "{}",
        stderr(&refused.output)
    );
    assert!(
        refused.output.stdout.is_empty(),
        "busy exec returned unexpected stdout"
    );
    assert!(
        !workload.work.join("B.end").exists(),
        "busy samples did not complete under continuous output"
    );

    let busy_ended = wait_for_stamp(&workload.work.join("B.end"));
    workload.await_free();
    thread::sleep(Duration::from_millis(100));
    workload.exec("exec_after_busy", "N", 0);
    assert!(
        !workload.work.join("R.start").exists(),
        "busy exec dispatched later"
    );
    let replay_end = workload.check_replay_to_live(pane);
    if let Some((child, _)) = &mut gui {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "desktop exited during the workload"
        );
    }
    emit(json!({
        "kind": "verified", "pane": pane, "busy_capture_rows": busy_rows,
        "busy_end_unix_ns": busy_ended.0, "last_display_line": row("L", RESULT_ROWS),
        "negotiated_replay_end": replay_end,
    }));
    if options.inspect {
        thread::sleep(Duration::from_secs(5));
    }
}

struct Workload {
    cli: PathBuf,
    daemon: Daemon,
    work: PathBuf,
    address: String,
}

impl Workload {
    fn observe(&self, pane: u64) -> PaneSession {
        PaneClient::at(self.daemon.pane_endpoint())
            .observe(pane, INITIAL_SIZE)
            .expect("observe the owned pane")
    }

    fn await_busy_output(&self, pane: u64) {
        let mut observer = self.observe(pane);
        let first = row("B", 1);
        let deadline = Instant::now() + LIMIT;
        let mut output = Vec::new();
        loop {
            if let DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) =
                recv(&mut observer, deadline)
            {
                let offset = output.len().saturating_sub(first.len() - 1);
                output.extend(bytes);
                if output[offset..]
                    .windows(first.len())
                    .any(|w| w == first.as_bytes())
                {
                    observer
                        .detach()
                        .expect("retire the readiness probe before sampling");
                    return;
                }
            }
        }
    }

    fn attach_gui(&self, app: &Path, workspace: &Value, pane: u64) -> (GuiChild, WinSize) {
        // An observational connection cannot resize the PTY. A Size different
        // from the CLI's initial dimensions proves the actual desktop attached.
        let mut observer = self.observe(pane);
        let mut gui = GuiChild(
            isolated_command(app, &self.daemon.config_dir)
                .arg("--config-dir")
                .arg(&self.daemon.config_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .expect("start the isolated desktop"),
        );
        let workspace = workspace["id"].as_str().unwrap().parse().unwrap();
        let hello = ControlHello::host_rpc("capture-exec-workload", "capture-exec-workload");
        let deadline = Instant::now() + super::READY_WITHIN;
        loop {
            assert!(
                gui.0.try_wait().unwrap().is_none(),
                "desktop exited before attaching"
            );
            let client =
                ControlClient::connect_at(&self.daemon.control_endpoint(), &hello).unwrap();
            let reply = client.request(ControlRequest::GuiOpen {
                path: None,
                workspace: Some(workspace),
            });
            match reply {
                Ok(ReplyOk::Bool(true)) => break,
                Ok(ReplyOk::Bool(false)) => {}
                other => panic!("opening the owned workspace: {other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "desktop never registered with the isolated daemon"
            );
            thread::sleep(Duration::from_millis(100));
        }
        loop {
            if let DaemonMsg::Size(size) = recv(&mut observer, deadline)
                && size != INITIAL_SIZE
            {
                assert!(
                    size.cols >= 40 && size.rows >= 12,
                    "fixture requires a grid of at least 40×12: {size:?}"
                );
                return (gui, size);
            }
        }
    }

    fn invoke(&self, args: &[&str]) -> Invocation {
        let mut command = isolated_command(&self.cli, &self.daemon.config_dir);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start_unix_ns = unix_nanos();
        let start = Instant::now();
        let mut child = command
            .spawn()
            .unwrap_or_else(|e| panic!("starting lyrel {args:?}: {e}"));
        let pid = child.id();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (output, elapsed) = thread::scope(|scope| {
            let stdout = scope.spawn(move || read_all(&mut stdout));
            let stderr = scope.spawn(move || read_all(&mut stderr));
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if start.elapsed() >= LIMIT {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("lyrel {args:?} exceeded {LIMIT:?}");
                }
                thread::sleep(Duration::from_millis(1));
            };
            let output = Output {
                status,
                stdout: stdout.join().unwrap(),
                stderr: stderr.join().unwrap(),
            };
            (output, start.elapsed())
        });
        Invocation {
            output,
            elapsed,
            start_unix_ns,
            pid,
        }
    }

    fn checked(&self, args: &[&str]) -> Value {
        let invocation = self.invoke(args);
        check_exit(&invocation.output, 0, args);
        serde_json::from_slice(&invocation.output.stdout)
            .unwrap_or_else(|e| panic!("lyrel {args:?} returned invalid JSON: {e}"))
    }

    fn await_free(&self) {
        let args = [
            "wait",
            &self.address,
            "--until",
            "free",
            "--interval",
            "50",
            "--timeout",
            "8",
            "--json",
        ];
        let report = self.checked(&args);
        assert_eq!(report["status"], "free", "{report}");
    }

    fn send(&self, phase: &str) {
        let args = [
            "send",
            &self.address,
            &producer_line(phase),
            "--enter",
            "--json",
        ];
        self.checked(&args);
    }

    fn capture(&self, name: &str, busy: bool) -> usize {
        let args = ["capture", &self.address, "--scrollback", "--json"];
        let invocation = self.invoke(&args);
        check_exit(&invocation.output, 0, &args);
        let result: Value =
            serde_json::from_slice(&invocation.output.stdout).expect("capture JSON");
        let text = result["text"].as_str().expect("capture text");
        let still_busy = busy && !self.work.join("B.end").exists();
        emit(invocation.record(
            name,
            json!({
                "bytes": result["bytes"], "still_busy": still_busy,
            }),
        ));
        assert_eq!(result["pane"].as_u64(), self.address[1..].parse().ok());
        assert_eq!(result["bytes"].as_u64(), Some(text.len() as u64));
        assert_eq!(rows(text, "S", false), SEED_ROWS);
        let seen = rows(text, "B", busy);
        if busy {
            assert!(
                seen > 0 && still_busy,
                "busy capture missed continuous output: {seen} rows, running={still_busy}"
            );
        } else {
            assert_eq!(seen, 0);
        }
        seen
    }

    fn exec(&self, name: &str, phase: &str, code: i32) {
        let args = [
            "exec",
            &self.address,
            "--json",
            "--timeout",
            "8",
            "--",
            &producer_line(phase),
        ];
        let invocation = self.invoke(&args);
        check_exit(&invocation.output, code, &args);
        let result: Value = serde_json::from_slice(&invocation.output.stdout).expect("exec JSON");
        let (dispatched, producer_pid) =
            read_stamp(&self.work.join(format!("{phase}.start"))).unwrap();
        let dispatch_ns = dispatched
            .checked_sub(invocation.start_unix_ns)
            .expect("system clock moved backwards");
        assert!(
            dispatch_ns <= invocation.elapsed.as_nanos(),
            "system clock moved during exec"
        );
        emit(invocation.record(
            name,
            json!({
                "dispatch_ns": dispatch_ns, "producer_pid": producer_pid, "exit": code,
                "bytes": result["bytes"], "backend_ms": result["elapsed_ms"],
            }),
        ));
        assert_eq!(result["pane"].as_u64(), self.address[1..].parse().ok());
        assert_eq!(result["exit"], code);
        assert_eq!(result["exit_code_known"], true);
        assert_eq!(result["timed_out"], false);
        assert_eq!(result["pane_exited"], false);
        let expected: Vec<_> = (1..=RESULT_ROWS).map(|n| row(phase, n)).collect();
        let actual = result["output"].as_str().unwrap();
        assert!(
            actual.lines().eq(expected.iter().map(String::as_str)),
            "incomplete/wrong exec stdout: {actual:?}"
        );
    }

    fn check_replay_to_live(&self, pane: u64) -> bool {
        let mut observer = PaneClient::at(self.daemon.pane_endpoint())
            .observe_with_replay_end(pane, INITIAL_SIZE)
            .expect("observe the replay-to-live transition");
        let negotiated = observer.has_replay_end();
        let mut ended = false;
        self.send("L");
        let deadline = Instant::now() + LIMIT;
        let mut replay = Vec::new();
        let mut live = Vec::new();
        let last = row("L", RESULT_ROWS);
        loop {
            match recv(&mut observer, deadline) {
                DaemonMsg::Snapshot(bytes) => {
                    assert!(
                        !ended && live.is_empty(),
                        "snapshot after the replay boundary"
                    );
                    replay.extend(bytes);
                }
                DaemonMsg::ReplayEnd => {
                    assert!(
                        negotiated && !ended && live.is_empty(),
                        "out-of-order replay boundary"
                    );
                    ended = true;
                }
                DaemonMsg::Output(bytes) => {
                    assert!(
                        !negotiated || ended,
                        "live bytes before the replay boundary"
                    );
                    live.extend(bytes);
                }
                DaemonMsg::Prompt {
                    at_prompt: true,
                    last_exit: Some(0),
                    ..
                } if live.windows(last.len()).any(|w| w == last.as_bytes()) => break,
                _ => {}
            }
        }
        let replay = String::from_utf8_lossy(&replay);
        let live = String::from_utf8_lossy(&live);
        assert_eq!(rows(&replay, "S", false), SEED_ROWS);
        assert_eq!(rows(&replay, "B", false), BUSY_ROWS);
        assert_eq!(rows(&replay, "E", false), RESULT_ROWS);
        assert_eq!(rows(&replay, "N", false), RESULT_ROWS);
        assert_eq!(rows(&replay, "L", false), 0);
        assert_eq!(rows(&live, "S", false), 0);
        assert_eq!(rows(&live, "B", false), 0);
        assert_eq!(rows(&live, "E", false), 0);
        assert_eq!(rows(&live, "N", false), 0);
        assert_eq!(rows(&live, "L", false), RESULT_ROWS);
        assert_eq!(ended, negotiated);
        let last_seed = replay.rfind(&row("S", SEED_ROWS)).unwrap();
        let first_exec = replay.find(&row("E", 1)).unwrap();
        let first_busy = replay.find(&row("B", 1)).unwrap();
        let last_busy = replay.rfind(&row("B", BUSY_ROWS)).unwrap();
        let next_exec = replay.find(&row("N", 1)).unwrap();
        assert!(last_seed < first_exec && first_exec < first_busy && last_busy < next_exec);
        negotiated
    }
}

struct GuiChild(Child);

impl Drop for GuiChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Invocation {
    output: Output,
    start_unix_ns: u128,
    elapsed: Duration,
    pid: u32,
}

impl Invocation {
    fn record(&self, action: &str, detail: Value) -> Value {
        json!({
            "kind": "sample", "action": action, "cli_pid": self.pid,
            "start_unix_ns": self.start_unix_ns, "completion_ns": self.elapsed.as_nanos(),
            "detail": detail,
        })
    }
}

fn executable(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).expect("resolve executable path");
    assert!(path.is_file(), "missing executable: {}", path.display());
    path
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn emit(record: Value) {
    println!("{record}");
    std::io::Write::flush(&mut std::io::stdout()).expect("emit workload record");
}

fn check_exit(output: &Output, code: i32, args: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "lyrel {args:?}: {}",
        stderr(output)
    );
    assert!(
        output.stderr.is_empty(),
        "lyrel {args:?}: {}",
        stderr(output)
    );
}

fn stderr(output: &Output) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(&output.stderr)
}

fn read_all(reader: &mut impl std::io::Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).expect("drain CLI pipe");
    bytes
}

fn recv(session: &mut PaneSession, deadline: Instant) -> DaemonMsg {
    let left = deadline.saturating_duration_since(Instant::now());
    assert!(!left.is_zero(), "pane stream deadline expired");
    session.set_recv_timeout(Some(left)).unwrap();
    match session.recv().expect("read pane stream") {
        DaemonMsg::Error(message) => panic!("pane stream error: {message}"),
        DaemonMsg::Exited { code } => panic!("owned pane exited: {code:?}"),
        msg => msg,
    }
}

fn producer_line(phase: &str) -> String {
    format!("./{FIXTURE} {PRODUCER_ARG} {phase}")
}

fn row(phase: &str, number: usize) -> String {
    format!("L436.{phase}.{number:04}|aB3cD5eF7gH9iJ1kL3|")
}

fn rows(text: &str, phase: &str, allow_incomplete_tail: bool) -> usize {
    let prefix = format!("L436.{phase}.");
    let mut count = 0;
    for (offset, _) in text.match_indices(&prefix) {
        let expected = row(phase, count + 1);
        let tail = &text[offset..];
        if allow_incomplete_tail && tail.len() < expected.len() && expected.starts_with(tail) {
            break;
        }
        assert!(
            tail.starts_with(&expected),
            "expected row {expected:?} at offset {offset}"
        );
        count += 1;
    }
    count
}

fn stamp(path: &str) {
    let timestamp = unix_nanos();
    // Publish only a complete record. The driver reads it while the producer is live.
    let writing = format!("{path}.tmp");
    fs::write(&writing, format!("{timestamp} {}", std::process::id())).unwrap();
    fs::rename(writing, path).unwrap();
}

fn read_stamp(path: &Path) -> Option<(u128, u32)> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let (timestamp, pid) = text.split_once(' ').expect("valid producer stamp");
            Some((timestamp.parse().unwrap(), pid.parse().unwrap()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("reading producer stamp {}: {e}", path.display()),
    }
}

fn wait_for_stamp(path: &Path) -> (u128, u32) {
    let deadline = Instant::now() + LIMIT;
    loop {
        if let Some(stamp) = read_stamp(path) {
            return stamp;
        }
        assert!(
            Instant::now() < deadline,
            "fixture did not write {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn produce(mut args: impl Iterator<Item = OsString>) -> i32 {
    use std::io::Write as _;

    let phase = args.next().expect("fixture phase");
    let phase = phase.to_str().expect("ASCII fixture phase");
    assert!(args.next().is_none());
    let (count, code) = match phase {
        "S" => (SEED_ROWS, 0),
        "B" => (BUSY_ROWS, 0),
        "E" => (RESULT_ROWS, 7),
        "N" | "L" | "R" => (RESULT_ROWS, 0),
        _ => panic!("unknown fixture phase {phase}"),
    };
    let mut stdout = std::io::stdout().lock();
    let started = Instant::now();
    if phase != "B" {
        stamp(&format!("{phase}.start"));
    }
    for n in 1..=count {
        if phase == "B" && n > 1 {
            let at = started + BUSY_INTERVAL * u32::try_from(n - 1).unwrap();
            thread::sleep(at.saturating_duration_since(Instant::now()));
        }
        writeln!(stdout, "{}", row(phase, n)).unwrap();
        if phase == "B" && n == 1 {
            stdout.flush().unwrap();
            stamp("B.start");
        }
    }
    stdout.flush().unwrap();
    if phase == "B" {
        stamp("B.end");
    }
    code
}
