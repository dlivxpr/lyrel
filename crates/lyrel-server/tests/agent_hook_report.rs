//! An agent hook run inside a pane reports to the daemon over its socket, not
//! through the pane's tty — where the agent's own output can cut into it.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::PaneClient;
use lyrel_core::core::cli_agent::{AGENT_EVENT_SENTINEL, AgentStatus};
use lyrel_core::daemon::protocol::{DaemonMsg, ShellSpec, WinSize};

const READY_WITHIN: Duration = Duration::from_secs(30);
const STREAM_WITHIN: Duration = Duration::from_secs(30);

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let dir = tempfile::TempDir::new().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
            .arg("--daemon")
            .arg("--config-dir")
            .arg(dir.path())
            .env("LYREL_DATA_DIR", dir.path())
            .env("LYREL_CONTROL_SOCK", dir.path().join("control.sock"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lyrel-server --daemon");
        let daemon = Daemon { child, dir };
        let deadline = Instant::now() + READY_WITHIN;
        while daemon.panes().version().is_err() {
            assert!(Instant::now() < deadline, "the daemon never came up");
            std::thread::sleep(Duration::from_millis(50));
        }
        daemon
    }

    fn panes(&self) -> PaneClient {
        PaneClient::at(self.dir.path().join("daemon.sock"))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hook_shell(pane_env: &str) -> ShellSpec {
    let hook = PathBuf::from(env!("CARGO_BIN_EXE_lyrel-server"));
    let command = format!(
        "printf '%s' '{{\"session_id\":\"s-1\",\"permission_mode\":\"plan\"}}' \
         | {pane_env} '{}' agent-hook kimi prompt-submit; echo hook_ran; exec cat",
        hook.display()
    );
    ShellSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), command],
        args_are_lyrel_defaults: false,
    }
}

/// Runs the hook in a fresh pane and returns what the pane printed up to the
/// hook finishing, and the agent status the daemon reported meanwhile.
fn run_hook(pane_env: &str) -> (Vec<u8>, Option<AgentStatus>) {
    let daemon = Daemon::start();
    let size = WinSize {
        cols: 100,
        rows: 30,
        cell_w: 8,
        cell_h: 16,
    };
    let mut session = daemon
        .panes()
        .spawn(None, size, Some(hook_shell(pane_env)), None, None)
        .expect("spawn the pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    let mut seen = Vec::new();
    let mut status = None;
    while !seen.windows(8).any(|w| w == b"hook_ran") {
        match session.recv().expect("the pane keeps streaming") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => seen.extend(bytes),
            DaemonMsg::AgentStatus(Some(s)) => status = Some(s.status),
            _ => {}
        }
    }
    // A report over the tty is parsed off the same output, just before it.
    while status.is_none() {
        match session.recv() {
            Ok(DaemonMsg::AgentStatus(Some(s))) => status = Some(s.status),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = session.kill();
    (seen, status)
}

fn carries_the_sequence(output: &[u8]) -> bool {
    let sentinel = AGENT_EVENT_SENTINEL.as_bytes();
    output.windows(sentinel.len()).any(|w| w == sentinel)
}

#[test]
fn a_hook_reports_over_the_socket_and_leaves_the_tty_alone() {
    let (output, status) = run_hook("");
    assert_eq!(status, Some(AgentStatus::Working));
    assert!(
        !carries_the_sequence(&output),
        "the report went through the tty: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn a_hook_the_daemon_turns_away_falls_back_to_the_tty() {
    // A pane id this daemon never handed out: the report is refused, and the
    // hook writes the sequence to its tty as before.
    let (output, status) = run_hook("LYREL_PANE=999999");
    assert!(
        carries_the_sequence(&output),
        "{:?} {status:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(status, Some(AgentStatus::Working));
}

#[test]
fn shell_output_marks_reach_pane_replay_and_machine_clients() {
    use lyrel_core::client::ControlClient;
    use lyrel_core::core::cli_agent::CLIAgent;
    use lyrel_core::core::machine::PaneSeed;
    use lyrel_core::daemon::control::{ControlHello, ControlRequest, ReplyOk};

    let daemon = Daemon::start();
    let directory = daemon.dir.path().join("work space");
    std::fs::create_dir(&directory).unwrap();
    let encoded_directory = directory.to_str().unwrap().replace('%', "%25").replace(' ', "%20");
    let script = format!(
        "printf 'shell_ready\\n'; read -r start; \
         printf '\\033]7;file://localhost{encoded_directory}\\007\\033]2;fact-title\\007\\033]133;B\\007\\033]133;C\\007'; \
         printf '\\033]777;notify;lyrel://cli-agent;%s\\007' \
         '{{\"agent\":\"omp\",\"event\":\"session-start\",\"session_id\":\"facts-session\"}}' \
         '{{\"agent\":\"omp\",\"event\":\"permission-request\",\"session_id\":\"facts-session\",\"request\":{{\"id\":\"facts-permission\",\"kind\":\"permission\"}}}}'; \
         read -r finish; exec /bin/sh -c \"$1\""
    );
    let finish_script = "printf '\\033]777;notify;lyrel://cli-agent;%s\\007\\033]2;\\007' \
        '{\"agent\":\"omp\",\"event\":\"session-end\",\"session_id\":\"facts-session\"}'; exec cat";
    let size = WinSize { cols: 100, rows: 30, cell_w: 8, cell_h: 16 };
    // A real PTY process with an agent argv identity, emitting the same formal
    // marks as a hook. No installed agent, model, or user's daemon is involved.
    let mut session = daemon.panes().spawn(
        Some(directory.clone()), size,
        Some(ShellSpec {
            program: "bash".into(),
            args: vec!["-c".into(), "exec -a omp /bin/sh -c \"$1\" lyrel-facts \"$2\"".into(), "lyrel-facts".into(), script, finish_script.into()],
            args_are_lyrel_defaults: false,
        }), None, None,
    ).unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let pane = session.pane_id();
    let deadline = Instant::now() + READY_WITHIN;
    let control = loop {
        match ControlClient::connect_at(&daemon.dir.path().join("control.sock"), &ControlHello::host_rpc("facts", "facts")) {
            Ok(control) => break control,
            Err(error) => {
                assert!(Instant::now() < deadline, "control endpoint unavailable: {error}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let workspace = match control.request(ControlRequest::WorkspaceCreate { name: Some("facts".into()), workspace: None }).unwrap() {
        ReplyOk::WorkspaceTree(workspace) => workspace.id,
        other => panic!("expected workspace, got {other:?}"),
    };
    control.request(ControlRequest::TabCreate { workspace, at: None, pane: PaneSeed::bare(pane), tab: None }).unwrap();
    let mut output = Vec::new();
    while !output.windows(11).any(|bytes| bytes == b"shell_ready") {
        if let DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) = session.recv().unwrap() { output.extend(bytes); }
    }
    session.input(b"start\n").unwrap();
    loop {
        if let DaemonMsg::AgentStatus(Some(state)) = session.recv().unwrap()
            && state.status == AgentStatus::Waiting
        {
            assert_eq!(state.session_id.as_deref(), Some("facts-session"));
            assert_eq!(state.pending_requests.len(), 1);
            break;
        }
    }
    let observed = |waiting: bool| {
        let deadline = Instant::now() + STREAM_WITHIN;
        loop {
            let ReplyOk::MachineTree(machine) = control.request(ControlRequest::MachineGet).unwrap() else { panic!("expected machine"); };
            let record = machine.panes.iter().find(|record| record.id == pane).unwrap();
            if waiting && record.agent.as_ref().is_some_and(|agent| agent.status == Some(AgentStatus::Waiting)) {
                assert_eq!(record.cwd.as_deref(), directory.to_str());
                assert_eq!(record.osc_title.as_deref(), Some("fact-title"));
                let agent = record.agent.as_ref().unwrap();
                assert_eq!(agent.agent, CLIAgent::OhMyPi);
                assert_eq!(agent.session_id.as_deref(), Some("facts-session"));
                break;
            }
            if !waiting && record.agent.is_none() && record.osc_title.is_none() { break; }
            assert!(Instant::now() < deadline, "facts did not land: {record:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    observed(true);
    session.detach().unwrap();
    let mut session = daemon.panes().attach(pane, size).unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let mut replayed_cwd = None;
    loop {
        match session.recv().unwrap() {
            DaemonMsg::Cwd(cwd) => replayed_cwd = Some(cwd),
            DaemonMsg::AgentStatus(Some(state)) => {
                assert_eq!(state.session_id.as_deref(), Some("facts-session"));
                assert_eq!(state.status, AgentStatus::Waiting);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(replayed_cwd.as_deref(), Some(directory.as_path()));
    session.input(b"finish\n").unwrap();
    loop {
        if matches!(session.recv().unwrap(), DaemonMsg::AgentStatus(None)) { break; }
    }
    observed(false);
    session.kill().unwrap();
}

#[test]
fn every_agent_result_is_refused_by_a_daemon_without_result_support() {
    use lyrel_core::daemon::protocol::{ClientMsg, DaemonVersion};
    use std::io::Write as _;
    use std::os::unix::net::UnixListener;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let directory = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(directory.path().join("daemon.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let server = std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    if matches!(ClientMsg::read(&mut stream), Ok(ClientMsg::Version)) {
                        DaemonMsg::Version(DaemonVersion {
                            protocol: 7,
                            build: "old-pane-v7".into(),
                            features: vec![],
                            instance: "old".into(),
                        })
                        .encode(&mut stream)
                        .unwrap();
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
    });
    let outputs = ["pi", "hermes", "kimi", "omp", "opencode"].map(|agent| {
        let mut hook = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
            .args(["agent-hook", agent, "tool-complete"])
            .env("LYREL_CONFIG_DIR", directory.path())
            .env("LYREL", "1")
            .env_remove("LYREL_PANE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        hook.stdin
            .take()
            .unwrap()
            .write_all(br#"{"result":{"outcome":"failed","reason":"provider unavailable"}}"#)
            .unwrap();
        (agent, hook.wait_with_output().unwrap())
    });
    stopped.store(true, Ordering::Relaxed);
    server.join().unwrap();
    for (agent, output) in outputs {
        assert_eq!(output.status.code(), Some(2), "{agent}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("agent-results"),
            "{agent}: {output:?}"
        );
    }
}
