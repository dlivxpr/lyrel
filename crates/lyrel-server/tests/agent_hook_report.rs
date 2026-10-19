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
