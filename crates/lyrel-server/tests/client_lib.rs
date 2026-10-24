use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::{ControlClient, PaneClient};
use lyrel_core::core::machine::{LayoutDelta, PaneSeed};
use lyrel_core::daemon::control::{ControlEvent, ControlHello, ControlRequest, ReplyOk, feature};
use lyrel_core::daemon::protocol::{DaemonMsg, PROTOCOL_VERSION, ShellSpec, WinSize};

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
        daemon.await_ready();
        daemon
    }

    fn pane_endpoint(&self) -> PathBuf {
        let file = if cfg!(windows) {
            "daemon.port"
        } else {
            "daemon.sock"
        };
        self.dir.path().join(file)
    }

    fn control_endpoint(&self) -> PathBuf {
        let file = if cfg!(windows) {
            "control.port"
        } else {
            "control.sock"
        };
        self.dir.path().join(file)
    }

    fn panes(&self) -> PaneClient {
        PaneClient::at(self.pane_endpoint())
    }

    fn control(&self, name: &str) -> ControlClient {
        ControlClient::connect_at(&self.control_endpoint(), &hello(name))
            .expect("control handshake with the spawned server")
    }

    fn await_ready(&self) {
        let deadline = Instant::now() + READY_WITHIN;
        loop {
            let control_up =
                ControlClient::connect_at(&self.control_endpoint(), &hello("probe")).is_ok();
            let panes_up = self.panes().version().is_ok();
            if control_up && panes_up {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "lyrel-server did not open its endpoints within {READY_WITHIN:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hello(name: &str) -> ControlHello {
    ControlHello::host_rpc(name, name)
}

fn size() -> WinSize {
    WinSize {
        cols: 100,
        rows: 30,
        cell_w: 8,
        cell_h: 16,
    }
}

#[test]
fn incompatible_pane_peers_are_refused_before_session_side_effects() {
    use lyrel_core::daemon::protocol::ClientMsg;
    use lyrel_core::daemon::transport;

    let daemon = Daemon::start();
    let mut legacy = transport::connect_endpoint_at(&daemon.pane_endpoint()).unwrap();
    legacy.set_read_timeout(Some(STREAM_WITHIN)).unwrap();
    ClientMsg::Spawn {
        cwd: None,
        size: size(),
        shell: Some(interactive_shell()),
        owner: None,
        workspace: None,
        restore: None,
        allow_remote_clipboard_write: false,
        await_claim: false,
    }
    .encode(&mut legacy)
    .unwrap();
    match DaemonMsg::read(&mut legacy).unwrap() {
        DaemonMsg::Error(message) => assert!(message.contains("handshake required"), "{message}"),
        other => panic!("legacy peer unexpectedly opened a session: {other:?}"),
    }
    assert!(
        daemon.panes().list().unwrap().is_empty(),
        "refused spawn created a shell"
    );

    let mut mismatch = transport::connect_endpoint_at(&daemon.pane_endpoint()).unwrap();
    mismatch.set_read_timeout(Some(STREAM_WITHIN)).unwrap();
    ClientMsg::Negotiate {
        protocol: PROTOCOL_VERSION - 1,
    }
    .encode(&mut mismatch)
    .unwrap();
    assert!(
        matches!(DaemonMsg::read(&mut mismatch).unwrap(), DaemonMsg::Version(version) if version.protocol == PROTOCOL_VERSION)
    );
    assert!(
        DaemonMsg::read(&mut mismatch).is_err(),
        "mismatched peer remained accepted"
    );
    assert!(daemon.panes().list().unwrap().is_empty());

    // An unnegotiated observer must not subscribe or gain an alternate route to
    // graphics frames. Matching production clients still open a usable shell.
    let mut session = daemon
        .panes()
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let id = session.pane_id();
    let mut observer = transport::connect_endpoint_at(&daemon.pane_endpoint()).unwrap();
    observer.set_read_timeout(Some(STREAM_WITHIN)).unwrap();
    ClientMsg::Observe {
        pane_id: id,
        size: size(),
    }
    .encode(&mut observer)
    .unwrap();
    assert!(
        matches!(DaemonMsg::read(&mut observer).unwrap(), DaemonMsg::Error(message) if message.contains("handshake required"))
    );
    session.input(b"echo negotiated-shell-alive\r").unwrap();
    let (output, _) = collect_until(&mut session, b"negotiated-shell-alive");
    assert!(windows_contain(&output, b"negotiated-shell-alive"));
    session.kill().unwrap();
}

fn one_shot_shell(command: &str) -> ShellSpec {
    if cfg!(windows) {
        ShellSpec {
            program: "cmd.exe".into(),
            args: vec!["/d".into(), "/c".into(), command.into()],
            args_are_lyrel_defaults: false,
        }
    } else {
        ShellSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), command.into()],
            args_are_lyrel_defaults: false,
        }
    }
}

fn interactive_shell() -> ShellSpec {
    if cfg!(windows) {
        ShellSpec {
            program: "cmd.exe".into(),
            args: vec!["/d".into()],
            args_are_lyrel_defaults: false,
        }
    } else {
        ShellSpec {
            program: "/bin/sh".into(),
            args: Vec::new(),
            args_are_lyrel_defaults: false,
        }
    }
}

fn seed(pane: u64) -> PaneSeed {
    PaneSeed {
        pane,
        cwd: Some("/home/me/proj".into()),
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

fn collect_until(
    session: &mut lyrel_core::client::PaneSession,
    marker: &[u8],
) -> (Vec<u8>, Option<Option<i32>>) {
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend_from_slice(&bytes);
                if windows_contain(&seen, marker) {
                    return (seen, None);
                }
            }
            Ok(DaemonMsg::Exited { code }) => return (seen, Some(code)),
            Ok(_) => {}
            Err(e) => panic!(
                "pane stream ended early: {e}; saw {:?}",
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

fn drain_until_exit(session: &mut lyrel_core::client::PaneSession) -> Vec<u8> {
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend_from_slice(&bytes);
            }
            Ok(DaemonMsg::Exited { .. }) => return seen,
            Ok(_) => {}
            Err(e) => panic!(
                "pane stream ended before Exited: {e}; saw {:?}",
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

// Explicit close deliberately does not emit Exited. Check the owned OS
// process, not just removal from the daemon's pane registry.
fn assert_process_exited(pid: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };

        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(ERROR_INVALID_PARAMETER as i32),
                "could not observe the owned shell {pid}"
            );
            return;
        }
        let result = unsafe { WaitForSingleObject(handle, STREAM_WITHIN.as_millis() as u32) };
        unsafe { CloseHandle(handle) };
        assert_eq!(result, WAIT_OBJECT_0, "closed shell {pid} is still running");
    }
    #[cfg(unix)]
    {
        let deadline = Instant::now() + STREAM_WITHIN;
        loop {
            if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "closed shell {pid} is still running"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn windows_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_pane_daemon_reports_its_dialect() {
    let daemon = Daemon::start();
    let version = daemon.panes().version().expect("query the version");
    assert_eq!(version.protocol, PROTOCOL_VERSION);
}

#[test]
fn control_requests_build_the_tree_and_events_reach_the_other_client() {
    let daemon = Daemon::start();
    let writer = daemon.control("writer");
    assert!(
        writer.hello().has_feature(feature::MACHINE_TREE),
        "features were {:?}",
        writer.hello().features
    );

    let watcher = daemon.control("watcher");
    watcher
        .request(ControlRequest::Ping)
        .expect("the watcher is live before the writer acts");

    let ws = match writer
        .request(ControlRequest::WorkspaceCreate {
            name: Some("api".into()),
            workspace: None,
        })
        .expect("create a workspace")
    {
        ReplyOk::WorkspaceTree(ws) => *ws,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    match writer
        .request(ControlRequest::TabCreate {
            workspace: ws.id,
            at: None,
            pane: seed(1),
            tab: None,
        })
        .expect("create a tab")
    {
        ReplyOk::TabTree(_) => {}
        other => panic!("expected TabTree, got {other:?}"),
    }

    let machine = match writer
        .request(ControlRequest::MachineGet)
        .expect("fetch the machine tree")
    {
        ReplyOk::MachineTree(m) => *m,
        other => panic!("expected MachineTree, got {other:?}"),
    };
    assert_eq!(machine.workspaces.len(), 1);
    assert_eq!(machine.workspaces[0].tabs[0].root.pane_ids(), vec![1]);

    let key = ws.id.to_string();
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "the watcher never saw the WorkspaceCreated delta for {key}"
        );
        match watcher.next_event(remaining) {
            Some(ControlEvent::Layout { workspace, delta })
                if workspace == key && matches!(delta, LayoutDelta::WorkspaceCreated { .. }) =>
            {
                break;
            }
            Some(_) => {}
            None => {}
        }
    }
}

#[test]
fn a_spawned_pane_streams_its_output_and_its_exit() {
    let daemon = Daemon::start();
    let mut session = daemon
        .panes()
        .spawn(
            None,
            size(),
            Some(one_shot_shell("echo lyrel_pane_roundtrip")),
            Some("client-lib-test".into()),
            None,
        )
        .expect("spawn a one-shot pane");
    assert_ne!(session.pane_id(), 0, "the daemon must name the pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    let seen = drain_until_exit(&mut session);
    assert!(
        windows_contain(&seen, b"lyrel_pane_roundtrip"),
        "output was {:?}",
        String::from_utf8_lossy(&seen)
    );
}

#[test]
fn a_one_shot_pane_reports_its_real_exit_code() {
    let daemon = Daemon::start();
    let mut session = daemon
        .panes()
        .spawn(
            None,
            size(),
            Some(one_shot_shell("exit 5")),
            Some("client-lib-test".into()),
            None,
        )
        .expect("spawn a failing one-shot pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    loop {
        match session.recv() {
            Ok(DaemonMsg::Exited { code }) => {
                assert_eq!(code, Some(5), "the child's code must ride the Exited frame");
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("pane stream ended before Exited: {e}"),
        }
    }
}

#[test]
fn input_reaches_the_shell_and_a_reattach_replays_it() {
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let mut session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn an interactive pane");
    let pane_id = session.pane_id();
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    session
        .input(b"echo lyrel_attach_replay\r")
        .expect("type into the pane");
    let (_, exit) = collect_until(&mut session, b"lyrel_attach_replay");
    assert!(exit.is_none(), "the shell must still be running");
    session.detach().expect("detach");

    let listed = panes.list().expect("list panes");
    let entry = listed
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("the detached pane is still listed");
    assert!(entry.alive, "the detached pane is still alive");

    let mut reattached = panes.attach(pane_id, size()).expect("reattach");
    reattached
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the replay reads");
    let (_, exit) = collect_until(&mut reattached, b"lyrel_attach_replay");
    assert!(exit.is_none(), "the replayed pane is still running");

    let refused = panes
        .attach(u64::MAX, size())
        .expect_err("attaching to a pane that never existed must fail");
    assert!(
        refused.to_string().contains("no such pane"),
        "the refusal was {refused}"
    );

    // This test covers attach replay rather than the streaming connection's
    // Kill command. Close that connection explicitly, then use the one-shot
    // PaneClient path for deterministic cleanup on loaded Windows runners.
    drop(reattached);
    panes.kill(pane_id).expect("kill the pane");
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let listed = panes.list().expect("list panes after the kill");
        let gone = !listed.iter().any(|p| p.pane_id == pane_id && p.alive);
        if gone {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "pane {pane_id} was still listed alive after Kill: {listed:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn pip_keeps_the_live_shell_across_replacement_fallback_and_reconnect() {
    use lyrel_core::core::machine::{Axis, Machine, PaneNode};

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let writer = daemon.control("pip-writer");
    let watcher = daemon.control("pip-watcher");
    watcher.request(ControlRequest::Ping).unwrap();
    let ws = match writer
        .request(ControlRequest::WorkspaceCreate {
            name: Some("pip".into()),
            workspace: None,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(ws) => ws.id,
        other => panic!("{other:?}"),
    };
    let mut session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let original = session.pane_id();
    writer
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(original),
            tab: None,
        })
        .unwrap();
    let get = |client: &ControlClient| -> Machine {
        match client.request(ControlRequest::MachineGet).unwrap() {
            ReplyOk::MachineTree(machine) => *machine,
            other => panic!("{other:?}"),
        }
    };
    let present = |pane, pip, fallback| {
        writer
            .request(ControlRequest::PaneSetPresentation {
                workspace: ws,
                pane,
                pip,
                fallback,
                restore: None,
            })
            .unwrap();
    };
    let initial = get(&writer);
    session
        .input(if cfg!(windows) {
            b"set lyrel_pip_state=retained\r"
        } else {
            b"lyrel_pip_state=retained\r"
        })
        .unwrap();
    present(original, true, None);
    let single = get(&watcher);
    let tab = &single.workspaces[0].tabs[0];
    assert_eq!(tab.root, initial.workspaces[0].tabs[0].root);
    assert_eq!(tab.presentation.pip().unwrap().pane(), original);
    assert_eq!(
        tab.visible_root(),
        None,
        "single-pane PiP does not create a filler shell"
    );
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let event = watcher
            .next_event(deadline.saturating_duration_since(Instant::now()))
            .expect("PiP delta");
        if matches!(event, ControlEvent::Layout { delta: LayoutDelta::TabRestructured { tab, pane: None }, .. }
            if tab.presentation.pip().is_some_and(|pip| pip.pane() == original))
        {
            break;
        }
        assert!(Instant::now() < deadline, "no presentation delta");
    }
    assert_eq!(panes.list().unwrap().iter().filter(|p| p.alive).count(), 1);
    assert!(
        writer
            .request(ControlRequest::PaneSetPresentation {
                workspace: ws,
                pane: u64::MAX,
                pip: true,
                fallback: None,
                restore: None,
            })
            .is_err()
    );
    assert_eq!(
        get(&writer).workspaces[0].tabs[0].presentation,
        tab.presentation
    );
    present(original, false, None);
    assert_eq!(
        get(&writer).workspaces[0].tabs[0],
        initial.workspaces[0].tabs[0],
        "ordinary restore only unhides and keeps the complete logical tree"
    );
    present(original, true, None);
    panes
        .send_input(
            original,
            if cfg!(windows) {
                b"echo PIP_%lyrel_pip_state%\r"
            } else {
                b"printf 'PIP_%s\\n' \"$lyrel_pip_state\"\r"
            },
        )
        .unwrap();
    assert!(collect_until(&mut session, b"PIP_retained").1.is_none());
    session.detach().unwrap();
    let mut captured = panes.attach(original, size()).unwrap();
    captured.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    assert!(
        collect_until(&mut captured, b"PIP_retained").1.is_none(),
        "capture/replay retains the original session"
    );

    let sibling_session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let sibling = sibling_session.pane_id();
    writer
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: original,
            axis: Axis::Vertical,
            ratio: 0.3,
            new: seed(sibling),
            first: false,
        })
        .unwrap();
    present(sibling, true, Some(original));
    assert_eq!(
        get(&writer).workspaces[0].tabs[0]
            .presentation
            .pip()
            .unwrap()
            .pane(),
        sibling
    );
    present(original, true, Some(sibling));
    let replacement_session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let replacement = replacement_session.pane_id();
    writer
        .request(ControlRequest::PaneReplace {
            workspace: ws,
            old: original,
            new: seed(replacement),
        })
        .unwrap();
    assert_eq!(
        get(&writer).workspaces[0].tabs[0]
            .presentation
            .pip()
            .unwrap()
            .pane(),
        replacement
    );
    let original_relationship = get(&writer).workspaces[0].tabs[0]
        .presentation
        .pip()
        .cloned();

    let fallback_session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let fallback = fallback_session.pane_id();
    writer
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: sibling,
            axis: Axis::Horizontal,
            ratio: 0.6,
            new: seed(fallback),
            first: false,
        })
        .unwrap();
    let other_session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let other = other_session.pane_id();
    writer
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: fallback,
            axis: Axis::Horizontal,
            ratio: 0.6,
            new: seed(other),
            first: false,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneClose {
            workspace: ws,
            pane: sibling,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneMove {
            workspace: ws,
            pane: replacement,
            to: other,
            axis: Axis::Horizontal,
            first: false,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: replacement,
            pip: true,
            fallback: Some(fallback),
            restore: original_relationship.clone(),
        })
        .unwrap();
    assert_eq!(
        get(&writer).workspaces[0].tabs[0].presentation.pip(),
        original_relationship.as_ref(),
        "same-pane synchronization restores original relationship after arbitrary reshaping"
    );
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let event = watcher
            .next_event(deadline.saturating_duration_since(Instant::now()))
            .expect("metadata-only delta");
        if matches!(event, ControlEvent::Layout { delta: LayoutDelta::TabRestructured { tab, pane: None }, .. }
            if tab.root.pane_ids() == vec![fallback, other, replacement]
                && tab.presentation.pip() == original_relationship.as_ref())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "same-pane metadata change was not broadcast"
        );
    }
    let persisted_restore = lyrel_core::core::machine::MachineStore::open(
        daemon
            .dir
            .path()
            .join(lyrel_core::core::machine::MACHINE_FILE),
    )
    .machine();
    assert_eq!(
        persisted_restore.workspaces[0].tabs[0].presentation.pip(),
        original_relationship.as_ref()
    );
    let mismatch = writer
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: other,
            pip: true,
            fallback: None,
            restore: original_relationship.clone(),
        })
        .unwrap_err();
    assert_eq!(mismatch.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        get(&writer).workspaces[0].tabs[0].presentation.pip(),
        original_relationship.as_ref()
    );
    let before_restore = panes.list().unwrap();
    present(replacement, false, Some(fallback));
    let restored = get(&writer);
    let tab = &restored.workspaces[0].tabs[0];
    assert!(tab.presentation.pip().is_none());
    assert_eq!(
        tab.root,
        PaneNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.6,
            a: Box::new(PaneNode::Split {
                axis: Axis::Vertical,
                ratio: 0.3,
                a: Box::new(PaneNode::Leaf { pane: replacement }),
                b: Box::new(PaneNode::Leaf { pane: fallback }),
            }),
            b: Box::new(PaneNode::Leaf { pane: other }),
        },
        "only the restored pane moves next to the chosen current fallback"
    );
    assert_eq!(
        panes
            .list()
            .unwrap()
            .iter()
            .map(|p| (p.pane_id, p.alive))
            .collect::<Vec<_>>(),
        before_restore
            .iter()
            .map(|p| (p.pane_id, p.alive))
            .collect::<Vec<_>>(),
        "presentation changes never close or spawn a shell"
    );
    present(replacement, true, Some(fallback));
    writer.close();
    let reconnected = daemon.control("pip-reconnected");
    let durable = get(&reconnected);
    assert_eq!(
        durable.workspaces[0].tabs[0]
            .presentation
            .pip()
            .unwrap()
            .pane(),
        replacement
    );
    let persisted = lyrel_core::core::machine::MachineStore::open(
        daemon
            .dir
            .path()
            .join(lyrel_core::core::machine::MACHINE_FILE),
    )
    .machine();
    assert_eq!(
        persisted.workspaces[0].tabs[0].presentation,
        durable.workspaces[0].tabs[0].presentation
    );
    reconnected
        .request(ControlRequest::PaneMove {
            workspace: ws,
            pane: replacement,
            to: other,
            axis: Axis::Horizontal,
            first: false,
        })
        .unwrap();
    let moved = get(&reconnected);
    assert_eq!(
        moved.workspaces[0].tabs[0]
            .presentation
            .pip()
            .unwrap()
            .pane(),
        replacement
    );
    assert_eq!(
        moved.workspaces[0].tabs[0].root.pane_ids(),
        vec![fallback, other, replacement]
    );
    assert_eq!(
        moved.workspaces[0].tabs[0]
            .visible_root()
            .unwrap()
            .pane_ids(),
        vec![fallback, other]
    );
    reconnected
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: other,
            pip: false,
            fallback: Some(fallback),
            restore: None,
        })
        .unwrap();
    assert_eq!(
        get(&reconnected).workspaces[0].tabs[0].presentation,
        moved.workspaces[0].tabs[0].presentation,
        "a stale restore cannot dismiss another pane's PiP"
    );
    reconnected
        .request(ControlRequest::PaneClose {
            workspace: ws,
            pane: replacement,
        })
        .unwrap();
    let closed = get(&reconnected);
    assert!(closed.workspaces[0].tabs[0].presentation.pip().is_none());
    assert!(!closed.workspaces[0].tabs[0].root.contains(replacement));
    assert_eq!(
        closed.workspaces[0].tabs[0].root.pane_ids(),
        vec![fallback, other]
    );
    panes
        .send_input(
            original,
            if cfg!(windows) {
                b"echo STILL_%lyrel_pip_state%\r"
            } else {
                b"printf 'STILL_%s\\n' \"$lyrel_pip_state\"\r"
            },
        )
        .unwrap();
    assert!(
        collect_until(&mut captured, b"STILL_retained").1.is_none(),
        "replacing tree identity never replaces the old live PTY implicitly"
    );
    drop((
        captured,
        sibling_session,
        replacement_session,
        fallback_session,
        other_session,
    ));
    for id in [original, sibling, replacement, fallback, other] {
        panes.kill(id).unwrap();
    }
}

#[test]
fn pip_restore_keeps_intervening_splits_without_adopting_a_later_move() {
    use lyrel_core::core::machine::{Axis, PaneNode};
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let control = daemon.control("pip-relationship");
    let ws = match control
        .request(ControlRequest::WorkspaceCreate {
            name: None,
            workspace: None,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(ws) => ws.id,
        other => panic!("{other:?}"),
    };
    let sessions = (0..3)
        .map(|_| {
            panes
                .spawn(None, size(), Some(interactive_shell()), None, None)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (a, b, c) = (
        sessions[0].pane_id(),
        sessions[1].pane_id(),
        sessions[2].pane_id(),
    );
    control
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(a),
            tab: None,
        })
        .unwrap();
    control
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Horizontal,
            ratio: 0.4,
            new: seed(b),
            first: false,
        })
        .unwrap();
    control
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: true,
            fallback: Some(a),
            restore: None,
        })
        .unwrap();
    control
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Vertical,
            ratio: 0.6,
            new: seed(c),
            first: false,
        })
        .unwrap();
    control
        .request(ControlRequest::PaneMove {
            workspace: ws,
            pane: b,
            to: c,
            axis: Axis::Vertical,
            first: false,
        })
        .unwrap();
    control
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: false,
            fallback: Some(c),
            restore: None,
        })
        .unwrap();
    let tree = match control
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(ws) => ws,
        other => panic!("{other:?}"),
    };
    let tab = &tree.tabs[0];
    assert!(tab.presentation.pip().is_none());
    assert_eq!(tab.root.pane_ids(), vec![a, b, c]);
    assert!(
        matches!(&tab.root, PaneNode::Split { axis: Axis::Vertical, a: restored, b: untouched, .. }
        if matches!(&**restored, PaneNode::Split { axis: Axis::Horizontal, a: original, b: returned, .. }
            if original.contains(a) && returned.contains(b)) && untouched.contains(c)),
        "restore rejoins the surviving original sibling without replaying its old subtree over the newer split"
    );
    assert_eq!(
        panes
            .list()
            .unwrap()
            .iter()
            .filter(|pane| pane.alive)
            .count(),
        3
    );
    drop(sessions);
    for pane in [a, b, c] {
        panes.kill(pane).unwrap();
    }
}

#[test]
fn mixed_owner_sidecar_survives_clients_without_colliding_or_overwriting_new_panes() {
    use lyrel_core::core::machine::{Axis, Tab};
    use lyrel_core::core::pane_sidecar::{ScopedPane, SidecarUpdate};
    use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};
    let local = Daemon::start();
    let remote = Daemon::start();
    let local_panes = local.panes();
    let remote_panes = remote.panes();
    let first = local.control("sidecar-first");
    let second = local.control("sidecar-second");
    let mut native = local_panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let mut remote_shell = remote_panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    native.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    remote_shell.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let local_id = native.pane_id();
    let remote_id = remote_shell.pane_id();
    assert_eq!(
        local_id, remote_id,
        "independent daemons can legitimately allocate the same integer"
    );
    let reference = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-host", 22),
        WorkspaceId::new(),
    );
    let mut tab = Tab::leaf(ScopedPane::workspace(remote_id));
    tab.root.split_leaf(
        ScopedPane::workspace(remote_id),
        ScopedPane::client_local(local_id),
        Axis::Horizontal,
        0.5,
        false,
    );
    tab.set_pip(
        Some(ScopedPane::client_local(local_id)),
        Some(ScopedPane::workspace(remote_id)),
    );
    let apply = |client: &ControlClient, base, desired, seeds| match client
        .request(ControlRequest::WorkspaceSidecarApply {
            remote: reference.clone(),
            updates: vec![SidecarUpdate::new(base, desired)],
            panes: seeds,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceSidecar(Some(snapshot)) => snapshot.into_parts().0,
        other => panic!("{other:?}"),
    };
    let saved = apply(&first, None, tab.clone(), vec![seed(local_id)]);
    assert_eq!(
        saved.tabs[0].root.pane_ids(),
        vec![local_id],
        "the local daemon owns only the local projection"
    );
    let full = saved.foreign_view.as_ref().unwrap().tabs()[0].clone();
    assert_eq!(
        full.root.pane_ids(),
        vec![
            ScopedPane::workspace(remote_id),
            ScopedPane::client_local(local_id)
        ]
    );
    let machine = match first.request(ControlRequest::MachineGet).unwrap() {
        ReplyOk::MachineTree(machine) => machine,
        other => panic!("{other:?}"),
    };
    assert_eq!(machine.user_workspaces().count(), 0);
    assert!(
        machine
            .workspaces
            .iter()
            .any(|workspace| workspace.id == saved.id)
    );
    assert!(machine.panes.iter().any(|pane| pane.id == local_id));
    assert!(
        local_panes
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.pane_id == local_id && pane.alive),
        "hiding a projection from workspace navigation must not hide its real pane"
    );
    local_panes
        .send_input(
            local_id,
            if cfg!(windows) {
                b"set SIDECAR_STATE=original\r"
            } else {
                b"SIDECAR_STATE=original\r"
            },
        )
        .unwrap();
    let added = local_panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let mut with_added = full.clone();
    with_added.root.split_leaf(
        ScopedPane::client_local(local_id),
        ScopedPane::client_local(added.pane_id()),
        Axis::Vertical,
        0.4,
        false,
    );
    apply(
        &first,
        Some(full.clone()),
        with_added,
        vec![seed(added.pane_id())],
    );
    let mut independently_restored = full.clone();
    independently_restored.set_pip(None, Some(ScopedPane::workspace(remote_id)));
    let merged = apply(&second, Some(full), independently_restored, Vec::new());
    let merged_tab = &merged.foreign_view.as_ref().unwrap().tabs()[0];
    assert!(
        merged_tab
            .root
            .contains(ScopedPane::client_local(added.pane_id())),
        "an older client operation cannot erase another client's newer pane"
    );
    assert!(merged_tab.presentation.pip().is_none());
    first.close();
    second.close();
    let reopened = local.control("sidecar-reopened");
    let fetched = match reopened
        .request(ControlRequest::WorkspaceSidecarGet {
            remote: reference.clone(),
        })
        .unwrap()
    {
        ReplyOk::WorkspaceSidecar(Some(snapshot)) => snapshot.into_parts().0,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        fetched.foreign_view.as_ref().unwrap().tabs()[0],
        *merged_tab
    );
    let geometry = lyrel_core::core::machine::PipGeometry::new(0.2, 0.3, 0.6, 0.5).unwrap();
    let mut parked = merged_tab.clone();
    parked.park(ScopedPane::client_local(local_id));
    parked.presentation.set_pip_geometry(Some(geometry));
    let parked = apply(&reopened, Some(merged_tab.clone()), parked, Vec::new());
    let parked = parked.foreign_view.as_ref().unwrap().tabs()[0].clone();
    let mut stale_remote_promotion = merged_tab.clone();
    stale_remote_promotion.set_pip(Some(ScopedPane::workspace(remote_id)), None);
    let combined = apply(
        &reopened,
        Some(merged_tab.clone()),
        stale_remote_promotion,
        Vec::new(),
    );
    let combined = &combined.foreign_view.as_ref().unwrap().tabs()[0];
    assert!(
        combined
            .presentation
            .is_parked(ScopedPane::client_local(local_id))
    );
    assert_eq!(combined.presentation.pip_geometry(), Some(geometry));
    assert_eq!(
        combined.presentation.pip().unwrap().pane(),
        ScopedPane::workspace(remote_id)
    );
    let mut restored = parked.clone();
    restored.restore_parked(
        ScopedPane::client_local(local_id),
        Some(ScopedPane::workspace(remote_id)),
    );
    let restored = apply(&reopened, Some(parked), restored, Vec::new());
    let restored = &restored.foreign_view.as_ref().unwrap().tabs()[0];
    assert!(
        !restored
            .presentation
            .is_parked(ScopedPane::client_local(local_id))
    );
    assert_eq!(
        restored.presentation.pip().unwrap().pane(),
        ScopedPane::workspace(remote_id)
    );
    local_panes
        .send_input(
            local_id,
            if cfg!(windows) {
                b"echo SIDECAR_%SIDECAR_STATE%\r"
            } else {
                b"printf 'SIDECAR_%s\\n' \"$SIDECAR_STATE\"\r"
            },
        )
        .unwrap();
    assert!(
        collect_until(&mut native, b"SIDECAR_original").1.is_none(),
        "the same local shell survives reopening clients"
    );
    remote_panes
        .send_input(remote_id, b"echo REMOTE_ROUTE_UNCHANGED\r")
        .unwrap();
    assert!(
        collect_until(&mut remote_shell, b"REMOTE_ROUTE_UNCHANGED")
            .1
            .is_none()
    );
    assert_eq!(
        local_panes
            .list()
            .unwrap()
            .iter()
            .filter(|pane| pane.alive)
            .count(),
        2
    );
    assert_eq!(
        remote_panes
            .list()
            .unwrap()
            .iter()
            .filter(|pane| pane.alive)
            .count(),
        1
    );
    drop((native, remote_shell, added));
    for pane in [
        local_id,
        merged_tab
            .root
            .pane_ids()
            .iter()
            .find(|pane| pane.owner().is_client_local() && pane.pane() != local_id)
            .unwrap()
            .pane(),
    ] {
        local_panes.kill(pane).unwrap();
    }
    remote_panes.kill(remote_id).unwrap();
}

fn sidecar_update(
    client: &ControlClient,
    remote: &lyrel_core::core::session::RemoteRef,
    updates: Vec<lyrel_core::core::pane_sidecar::SidecarUpdate>,
    panes: Vec<PaneSeed>,
) -> lyrel_core::core::machine::Workspace {
    match client
        .request(ControlRequest::WorkspaceSidecarApply {
            remote: remote.clone(),
            updates,
            panes,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceSidecar(Some(snapshot)) => snapshot.into_parts().0,
        other => panic!("{other:?}"),
    }
}

#[test]
fn sidecar_closed_tab_rejects_stale_reconcile_before_transferring_panes() {
    use lyrel_core::core::machine::{Axis, Tab};
    use lyrel_core::core::pane_sidecar::{ScopedPane, SidecarUpdate};
    use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};

    let daemon = Daemon::start();
    let client = daemon.control("sidecar-stale");
    let remote = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-host", 22),
        WorkspaceId::new(),
    );
    let closed = Tab::leaf(ScopedPane::workspace(41));
    let donor = Tab::leaf(ScopedPane::workspace(42));
    sidecar_update(
        &client,
        &remote,
        vec![
            SidecarUpdate::new(None, closed.clone()),
            SidecarUpdate::new(None, donor.clone()),
            SidecarUpdate::close_tab(closed.id),
        ],
        Vec::new(),
    );
    let mut stale = closed.clone();
    stale.root.split_leaf(
        ScopedPane::workspace(41),
        ScopedPane::workspace(42),
        Axis::Horizontal,
        0.5,
        false,
    );
    stale.set_pip(Some(ScopedPane::workspace(41)), None);
    let saved = sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::new(Some(closed.clone()), stale).with_transfer()],
        Vec::new(),
    );
    let tabs = saved.foreign_view.as_ref().unwrap().tabs();
    assert!(!tabs.iter().any(|tab| tab.id == closed.id));
    assert!(
        tabs.iter()
            .any(|tab| tab.id == donor.id && tab.root.contains(ScopedPane::workspace(42)))
    );
    let created = sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::new(None, closed.clone())],
        Vec::new(),
    );
    assert!(
        created
            .foreign_view
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .any(|tab| tab.id == closed.id)
    );
}

#[test]
fn sidecar_pane_close_preserves_remote_and_native_siblings_and_accepts_later_edits() {
    use lyrel_core::core::machine::{Axis, Tab};
    use lyrel_core::core::pane_sidecar::{ScopedPane, SidecarUpdate};
    use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};

    let local = Daemon::start();
    let remote_daemon = Daemon::start();
    let panes = local.panes();
    let remote_panes = remote_daemon.panes();
    let client = local.control("sidecar-pane-close");
    let native = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let local_id = native.pane_id();
    let mut sibling = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let sibling_id = sibling.pane_id();
    let mut remote_shell = remote_panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let remote_id = remote_shell.pane_id();
    assert_eq!(remote_id, local_id);
    sibling.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    remote_shell.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let remote = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-host", 22),
        WorkspaceId::new(),
    );
    let mut tab = Tab::leaf(ScopedPane::workspace(remote_id));
    tab.root.split_leaf(
        ScopedPane::workspace(remote_id),
        ScopedPane::client_local(local_id),
        Axis::Horizontal,
        0.5,
        false,
    );
    tab.root.split_leaf(
        ScopedPane::client_local(local_id),
        ScopedPane::client_local(sibling_id),
        Axis::Vertical,
        0.5,
        false,
    );
    tab.set_pip(
        Some(ScopedPane::client_local(local_id)),
        Some(ScopedPane::workspace(remote_id)),
    );
    tab.park(ScopedPane::workspace(remote_id));
    let saved = sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::new(None, tab.clone())],
        vec![seed(local_id), seed(sibling_id)],
    );
    let removed = match client
        .request(ControlRequest::PaneClose {
            workspace: saved.id,
            pane: local_id,
        })
        .unwrap()
    {
        ReplyOk::Panes(panes) => panes,
        other => panic!("{other:?}"),
    };
    assert!(removed.contains(&local_id));
    let snapshot = match client
        .request(ControlRequest::WorkspaceSidecarGet {
            remote: remote.clone(),
        })
        .unwrap()
    {
        ReplyOk::WorkspaceSidecar(Some(snapshot)) => snapshot.into_parts().0,
        other => panic!("{other:?}"),
    };
    let full = &snapshot.foreign_view.as_ref().unwrap().tabs()[0];
    assert!(!full.root.contains(ScopedPane::client_local(local_id)));
    assert!(full.root.contains(ScopedPane::workspace(remote_id)));
    assert!(full.root.contains(ScopedPane::client_local(sibling_id)));
    assert!(full.presentation.pip().is_none());
    assert!(
        full.presentation
            .is_parked(ScopedPane::workspace(remote_id))
    );
    assert_eq!(snapshot.tabs[0].root.pane_ids(), vec![sibling_id]);

    // A delayed client edit must not reintroduce the now-closed native leaf.
    let mut edited = tab.clone();
    edited.restore_parked(
        ScopedPane::workspace(remote_id),
        Some(ScopedPane::client_local(sibling_id)),
    );
    let updated = sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::new(Some(tab), edited)],
        Vec::new(),
    );
    let full = &updated.foreign_view.as_ref().unwrap().tabs()[0];
    assert!(!full.root.contains(ScopedPane::client_local(local_id)));
    assert!(
        !full
            .presentation
            .is_parked(ScopedPane::workspace(remote_id))
    );
    assert!(full.root.contains(ScopedPane::client_local(sibling_id)));
    panes
        .send_input(sibling_id, b"echo SIDECAR_SIBLING_ALIVE\r")
        .unwrap();
    assert!(
        collect_until(&mut sibling, b"SIDECAR_SIBLING_ALIVE")
            .1
            .is_none()
    );
    remote_panes
        .send_input(remote_id, b"echo SIDECAR_REMOTE_ALIVE\r")
        .unwrap();
    assert!(
        collect_until(&mut remote_shell, b"SIDECAR_REMOTE_ALIVE")
            .1
            .is_none()
    );
    client
        .request(ControlRequest::PaneClose {
            workspace: saved.id,
            pane: sibling_id,
        })
        .unwrap();
    let remaining = match client
        .request(ControlRequest::WorkspaceSidecarGet {
            remote: remote.clone(),
        })
        .unwrap()
    {
        ReplyOk::WorkspaceSidecar(Some(snapshot)) => snapshot.into_parts().0,
        other => panic!("{other:?}"),
    };
    assert!(remaining.tabs.is_empty());
    let remote_only = &remaining.foreign_view.as_ref().unwrap().tabs()[0];
    assert_eq!(
        remote_only.root.pane_ids(),
        vec![ScopedPane::workspace(remote_id)]
    );
    let mut parked_remote = remote_only.clone();
    parked_remote.park(ScopedPane::workspace(remote_id));
    let updated = sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::new(Some(remote_only.clone()), parked_remote)],
        Vec::new(),
    );
    assert!(
        updated.foreign_view.as_ref().unwrap().tabs()[0]
            .presentation
            .is_parked(ScopedPane::workspace(remote_id))
    );
    drop((native, sibling, remote_shell));
    for pane in removed.into_iter().chain([sibling_id]) {
        panes.kill(pane).unwrap();
    }
    remote_panes.kill(remote_id).unwrap();
}

#[test]
fn sidecar_explicit_close_terminates_only_unheld_local_shells_not_transferred_panes() {
    use lyrel_core::core::machine::{Axis, Tab};
    use lyrel_core::core::pane_sidecar::{ScopedPane, SidecarUpdate};
    use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};

    let local = Daemon::start();
    let remote_daemon = Daemon::start();
    let panes = local.panes();
    let remote_panes = remote_daemon.panes();
    let client = local.control("sidecar-explicit-close");
    let mut closing = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let closing_id = closing.pane_id();
    let mut moved = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let moved_id = moved.pane_id();
    let mut unrelated = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let unrelated_id = unrelated.pane_id();
    let mut remote_shell = remote_panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let remote_id = remote_shell.pane_id();
    assert_eq!(closing_id, remote_id);
    for session in [&mut closing, &mut moved, &mut unrelated, &mut remote_shell] {
        session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    }
    let shell_pid = |pane| {
        panes
            .procs(pane)
            .unwrap()
            .procs
            .into_iter()
            .find(|process| process.depth == 0)
            .expect("live pane reports its shell process")
            .pid
    };
    let closing_pid = shell_pid(closing_id);
    let moved_pid = shell_pid(moved_id);
    let remote = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-host", 22),
        WorkspaceId::new(),
    );
    let mut old = Tab::leaf(ScopedPane::workspace(remote_id));
    old.root.split_leaf(
        ScopedPane::workspace(remote_id),
        ScopedPane::client_local(closing_id),
        Axis::Horizontal,
        0.5,
        false,
    );
    old.root.split_leaf(
        ScopedPane::client_local(closing_id),
        ScopedPane::client_local(moved_id),
        Axis::Vertical,
        0.5,
        false,
    );
    let target = Tab::leaf(ScopedPane::workspace(999));
    sidecar_update(
        &client,
        &remote,
        vec![
            SidecarUpdate::new(None, old.clone()),
            SidecarUpdate::new(None, target.clone()),
        ],
        vec![seed(closing_id), seed(moved_id)],
    );
    let mut transferred = target.clone();
    transferred.root.split_leaf(
        ScopedPane::workspace(999),
        ScopedPane::client_local(moved_id),
        Axis::Horizontal,
        0.5,
        false,
    );
    sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::new(Some(target), transferred.clone()).with_transfer()],
        Vec::new(),
    );
    panes
        .send_input(moved_id, b"echo SIDECAR_TRANSFER_SURVIVED\r")
        .unwrap();
    assert!(
        collect_until(&mut moved, b"SIDECAR_TRANSFER_SURVIVED")
            .1
            .is_none()
    );

    let closed = sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::close_tab(old.id)],
        Vec::new(),
    );
    assert_process_exited(closing_pid);
    assert!(
        panes.attach(closing_id, size()).is_err(),
        "a closed shell cannot accept another attachment or input"
    );
    assert!(
        !panes
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.pane_id == closing_id && pane.alive)
    );
    let machine = match client.request(ControlRequest::MachineGet).unwrap() {
        ReplyOk::MachineTree(machine) => machine,
        other => panic!("{other:?}"),
    };
    assert!(!machine.panes.iter().any(|pane| pane.id == closing_id));
    assert!(
        closed
            .foreign_view
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .any(|tab| tab.root.contains(ScopedPane::client_local(moved_id)))
    );
    panes
        .send_input(moved_id, b"echo SIDECAR_MOVED_STILL_ALIVE\r")
        .unwrap();
    assert!(
        collect_until(&mut moved, b"SIDECAR_MOVED_STILL_ALIVE")
            .1
            .is_none()
    );
    panes
        .send_input(unrelated_id, b"echo SIDECAR_UNRELATED_ALIVE\r")
        .unwrap();
    assert!(
        collect_until(&mut unrelated, b"SIDECAR_UNRELATED_ALIVE")
            .1
            .is_none()
    );
    remote_panes
        .send_input(remote_id, b"echo SIDECAR_REMOTE_NOT_CLOSED\r")
        .unwrap();
    assert!(
        collect_until(&mut remote_shell, b"SIDECAR_REMOTE_NOT_CLOSED")
            .1
            .is_none()
    );
    // Closing a tab does not end a pane retained by another tab in the same
    // authoritative transaction, even when the close precedes the new owner.
    let retained = Tab::leaf(ScopedPane::client_local(moved_id));
    sidecar_update(
        &client,
        &remote,
        vec![
            SidecarUpdate::close_tab(transferred.id),
            SidecarUpdate::new(None, retained.clone()),
        ],
        Vec::new(),
    );
    panes
        .send_input(moved_id, b"echo SIDECAR_REHELD_ALIVE\r")
        .unwrap();
    assert!(
        collect_until(&mut moved, b"SIDECAR_REHELD_ALIVE")
            .1
            .is_none()
    );
    sidecar_update(
        &client,
        &remote,
        vec![SidecarUpdate::close_tab(retained.id)],
        Vec::new(),
    );
    assert_process_exited(moved_pid);
    drop((closing, moved, unrelated, remote_shell));
    panes.kill(unrelated_id).unwrap();
    remote_panes.kill(remote_id).unwrap();
}

#[test]
fn sidecar_stale_transfer_preserves_a_pane_moved_by_another_client() {
    use lyrel_core::core::machine::{Axis, Tab};
    use lyrel_core::core::pane_sidecar::{ScopedPane, SidecarUpdate};
    use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let first = daemon.control("sidecar-stale-transfer-first");
    let second = daemon.control("sidecar-stale-transfer-second");
    let mut moved = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    let mut added = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    moved.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    added.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let moved_id = moved.pane_id();
    let added_id = added.pane_id();
    let remote = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-host", 22),
        WorkspaceId::new(),
    );
    let mut a = Tab::leaf(ScopedPane::workspace(500));
    a.root.split_leaf(
        ScopedPane::workspace(500),
        ScopedPane::client_local(moved_id),
        Axis::Horizontal,
        0.5,
        false,
    );
    let b = Tab::leaf(ScopedPane::workspace(501));
    sidecar_update(
        &first,
        &remote,
        vec![
            SidecarUpdate::new(None, a.clone()),
            SidecarUpdate::new(None, b.clone()),
        ],
        vec![seed(moved_id)],
    );
    let mut destination = b.clone();
    destination.root.split_leaf(
        ScopedPane::workspace(501),
        ScopedPane::client_local(moved_id),
        Axis::Vertical,
        0.4,
        false,
    );
    destination.set_pip(Some(ScopedPane::client_local(moved_id)), None);
    sidecar_update(
        &second,
        &remote,
        vec![SidecarUpdate::new(Some(b.clone()), destination).with_transfer()],
        Vec::new(),
    );

    // The first client's unchanged P leaf is stale, not a request to move P
    // back. Only its newly added Q leaf has transfer authority.
    let mut delayed = a.clone();
    delayed.root.split_leaf(
        ScopedPane::workspace(500),
        ScopedPane::client_local(added_id),
        Axis::Vertical,
        0.6,
        false,
    );
    let saved = sidecar_update(
        &first,
        &remote,
        vec![SidecarUpdate::new(Some(a.clone()), delayed).with_transfer()],
        vec![seed(added_id)],
    );
    let tabs = saved.foreign_view.as_ref().unwrap().tabs();
    let current_a = tabs.iter().find(|tab| tab.id == a.id).unwrap();
    let current_b = tabs.iter().find(|tab| tab.id == b.id).unwrap();
    assert!(current_a.root.contains(ScopedPane::workspace(500)));
    assert!(current_a.root.contains(ScopedPane::client_local(added_id)));
    assert!(!current_a.root.contains(ScopedPane::client_local(moved_id)));
    assert!(current_b.root.contains(ScopedPane::client_local(moved_id)));
    assert_eq!(
        current_b.presentation.pip().unwrap().pane(),
        ScopedPane::client_local(moved_id)
    );
    panes
        .send_input(moved_id, b"echo SIDECAR_MOVED_OWNER_RETAINED\r")
        .unwrap();
    assert!(
        collect_until(&mut moved, b"SIDECAR_MOVED_OWNER_RETAINED")
            .1
            .is_none()
    );
    panes
        .send_input(added_id, b"echo SIDECAR_NEW_LEAF_ALIVE\r")
        .unwrap();
    assert!(
        collect_until(&mut added, b"SIDECAR_NEW_LEAF_ALIVE")
            .1
            .is_none()
    );
    drop((moved, added));
    panes.kill(moved_id).unwrap();
    panes.kill(added_id).unwrap();
}

fn parking_workspace(client: &ControlClient) -> lyrel_core::core::session::WorkspaceId {
    match client
        .request(ControlRequest::WorkspaceCreate {
            name: Some("parking".into()),
            workspace: None,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(workspace) => workspace.id,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    }
}

fn parking_tree(
    client: &ControlClient,
    workspace: lyrel_core::core::session::WorkspaceId,
) -> lyrel_core::core::machine::Workspace {
    match client
        .request(ControlRequest::WorkspaceTree { workspace })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(workspace) => *workspace,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    }
}

fn parking_shell(
    daemon: &Daemon,
    workspace: lyrel_core::core::session::WorkspaceId,
) -> lyrel_core::client::PaneSession {
    let session = daemon
        .panes()
        .spawn(
            None,
            size(),
            Some(interactive_shell()),
            Some(workspace.to_string()),
            None,
        )
        .unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    session
}

fn parking_wait_for_deck(
    watcher: &ControlClient,
    workspace: lyrel_core::core::session::WorkspaceId,
    expected: &[u64],
) {
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "parking change was not broadcast");
        if let Some(ControlEvent::Layout {
            workspace: changed,
            delta: LayoutDelta::TabRestructured { tab, pane: None },
        }) = watcher.next_event(remaining)
        {
            let mut parked = tab
                .presentation
                .parked()
                .iter()
                .map(|entry| entry.pane())
                .collect::<Vec<_>>();
            parked.sort_unstable();
            if changed == workspace.to_string() && parked == expected {
                return;
            }
        }
    }
}

#[test]
fn parking_and_chosen_restore_keep_the_same_live_shells_for_every_client() {
    use lyrel_core::core::machine::{Axis, MACHINE_FILE, MachineStore};

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let writer = daemon.control("parking-writer");
    let watcher = daemon.control("parking-watcher");
    watcher.request(ControlRequest::Ping).unwrap();
    let ws = parking_workspace(&writer);
    let mut first = parking_shell(&daemon, ws);
    let second = parking_shell(&daemon, ws);
    let (a, b) = (first.pane_id(), second.pane_id());
    writer
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(a),
            tab: None,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Horizontal,
            ratio: 0.4,
            new: seed(b),
            first: false,
        })
        .unwrap();
    first
        .input(if cfg!(windows) {
            b"set lyrel_park_state=retained\recho BEFORE_%lyrel_park_state%\r"
        } else {
            b"lyrel_park_state=retained\rprintf 'BEFORE_%s\\n' \"$lyrel_park_state\"\r"
        })
        .unwrap();
    assert!(collect_until(&mut first, b"BEFORE_retained").1.is_none());

    writer
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: a,
            restore: None,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: true,
            fallback: None,
            restore: None,
        })
        .unwrap();
    writer
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: None,
        })
        .unwrap();
    let mut expected = vec![a, b];
    expected.sort_unstable();
    parking_wait_for_deck(&watcher, ws, &expected);
    let all_parked = parking_tree(&watcher, ws);
    assert_eq!(
        all_parked.tabs.len(),
        1,
        "parking the last pane retains its tab"
    );
    let tab = &all_parked.tabs[0];
    assert!(tab.visible_root().is_none());
    assert!(tab.presentation.pip().is_none(), "PiP moves into the deck");
    assert!(
        tab.root.contains(a) && tab.root.contains(b),
        "both panes remain discoverable"
    );
    let listed = panes.list().unwrap();
    assert_eq!(listed.len(), 2, "all parked never starts a filler shell");
    for id in [a, b] {
        let pane = listed.iter().find(|pane| pane.pane_id == id).unwrap();
        assert!(pane.alive);
        assert_eq!(pane.owner.as_deref(), Some(ws.to_string().as_str()));
    }
    let persisted = MachineStore::open(daemon.dir.path().join(MACHINE_FILE)).machine();
    assert_eq!(
        persisted.workspaces[0].tabs[0].presentation,
        tab.presentation
    );
    first.detach().unwrap();
    writer.close();
    let reconnected = daemon.control("parking-reconnected");
    assert_eq!(
        parking_tree(&reconnected, ws).tabs[0].presentation,
        tab.presentation
    );
    let mut replay = panes.attach(a, size()).unwrap();
    replay.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    assert!(collect_until(&mut replay, b"BEFORE_retained").1.is_none());
    panes
        .send_input(
            a,
            if cfg!(windows) {
                b"echo PARKED_%lyrel_park_state%\r"
            } else {
                b"printf 'PARKED_%s\\n' \"$lyrel_park_state\"\r"
            },
        )
        .unwrap();
    assert!(collect_until(&mut replay, b"PARKED_retained").1.is_none());

    reconnected
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: b,
            fallback: None,
        })
        .unwrap();
    parking_wait_for_deck(&watcher, ws, &[a]);
    let chosen = parking_tree(&watcher, ws);
    assert_eq!(chosen.tabs[0].visible_root().unwrap().pane_ids(), vec![b]);
    assert!(chosen.tabs[0].presentation.is_parked(a));
    assert!(!chosen.tabs[0].presentation.is_parked(b));
    assert!(
        chosen.tabs[0].presentation.pip().is_none(),
        "a parked PiP restores tiled"
    );
    reconnected
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: Some(b),
        })
        .unwrap();
    parking_wait_for_deck(&watcher, ws, &[]);
    assert_eq!(
        parking_tree(&watcher, ws).tabs[0]
            .visible_root()
            .unwrap()
            .pane_ids(),
        vec![a, b]
    );
    panes
        .send_input(
            a,
            if cfg!(windows) {
                b"echo RESTORED_%lyrel_park_state%\r"
            } else {
                b"printf 'RESTORED_%s\\n' \"$lyrel_park_state\"\r"
            },
        )
        .unwrap();
    assert!(collect_until(&mut replay, b"RESTORED_retained").1.is_none());
    assert_eq!(panes.list().unwrap().len(), 2);
    drop((replay, second));
    for id in [a, b] {
        panes.kill(id).unwrap();
    }
}

#[test]
fn parked_restore_preserves_intervening_splits_then_uses_a_live_fallback() {
    use lyrel_core::core::machine::Axis;

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let client = daemon.control("parking-relationship");
    let ws = parking_workspace(&client);
    let sessions = (0..4)
        .map(|_| parking_shell(&daemon, ws))
        .collect::<Vec<_>>();
    let (a, b, c, d) = (
        sessions[0].pane_id(),
        sessions[1].pane_id(),
        sessions[2].pane_id(),
        sessions[3].pane_id(),
    );
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
            ratio: 0.4,
            new: seed(b),
            first: false,
        })
        .unwrap();
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Vertical,
            ratio: 0.6,
            new: seed(c),
            first: false,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: c,
            axis: Axis::Vertical,
            ratio: 0.7,
            new: seed(d),
            first: false,
        })
        .unwrap();
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: d,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneMove {
            workspace: ws,
            pane: b,
            to: c,
            axis: Axis::Vertical,
            first: false,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: b,
            fallback: Some(c),
        })
        .unwrap();
    let restored = parking_tree(&client, ws);
    assert_eq!(
        restored.tabs[0].visible_root().unwrap().pane_ids(),
        vec![a, b, c],
        "the chosen pane returns beside its original sibling, before the newer split"
    );
    assert!(restored.tabs[0].presentation.is_parked(d));
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneClose {
            workspace: ws,
            pane: a,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: b,
            fallback: Some(c),
        })
        .unwrap();
    let fallback = parking_tree(&client, ws);
    assert_eq!(
        fallback.tabs[0].visible_root().unwrap().pane_ids(),
        vec![c, b],
        "an invalid original sibling falls back beside the current surviving pane"
    );
    assert!(fallback.tabs[0].presentation.is_parked(d));
    assert_eq!(
        panes
            .list()
            .unwrap()
            .iter()
            .filter(|pane| pane.alive)
            .count(),
        4,
        "layout edits never stop or replace any of the four sessions"
    );
    drop(sessions);
    for id in [a, b, c, d] {
        panes.kill(id).unwrap();
    }
}

#[test]
fn parked_exit_and_explicit_close_never_revive_or_close_another_session() {
    use lyrel_core::core::machine::Axis;

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let client = daemon.control("parking-lifecycle");
    let ws = parking_workspace(&client);
    let mut exiting = parking_shell(&daemon, ws);
    let mut survivor = parking_shell(&daemon, ws);
    let (a, b) = (exiting.pane_id(), survivor.pane_id());
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
    for pane in [a, b] {
        client
            .request(ControlRequest::PanePark {
                workspace: ws,
                pane,
                restore: None,
            })
            .unwrap();
    }
    exiting.input(b"exit 7\r").unwrap();
    drain_until_exit(&mut exiting);
    let deadline = Instant::now() + STREAM_WITHIN;
    while panes
        .list()
        .unwrap()
        .iter()
        .any(|pane| pane.pane_id == a && pane.alive)
    {
        assert!(
            Instant::now() < deadline,
            "the exited shell was still reported live"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: None,
        })
        .unwrap();
    assert!(
        !panes
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.pane_id == a && pane.alive),
        "restoring an exited view must not respawn its process"
    );
    assert!(parking_tree(&client, ws).tabs[0].presentation.is_parked(b));
    client
        .request(ControlRequest::PaneClose {
            workspace: ws,
            pane: a,
        })
        .unwrap();
    let stale = client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: Some(b),
        })
        .unwrap_err();
    assert_eq!(stale.kind(), std::io::ErrorKind::NotFound);
    let after_close = parking_tree(&client, ws);
    assert!(!after_close.tabs[0].root.contains(a));
    assert!(!after_close.tabs[0].presentation.is_parked(a));
    assert!(after_close.tabs[0].presentation.is_parked(b));
    assert!(after_close.tabs[0].visible_root().is_none());
    assert!(
        panes
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.pane_id == b && pane.alive)
    );
    survivor
        .input(if cfg!(windows) {
            b"echo PARK_SURVIVOR_%LYREL_PANE%\r"
        } else {
            b"printf 'PARK_SURVIVOR_%s\\n' \"$LYREL_PANE\"\r"
        })
        .unwrap();
    assert!(
        collect_until(&mut survivor, format!("PARK_SURVIVOR_{b}").as_bytes())
            .1
            .is_none(),
        "stale restore of the exited pane must not close the other parked shell"
    );
    drop((exiting, survivor));
    panes.kill(b).unwrap();
}

#[test]
fn a_rebuilt_tree_preserves_the_original_parked_relationship_on_the_wire() {
    use lyrel_core::core::machine::{Axis, MACHINE_FILE, MachineStore};

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let writer = daemon.control("parking-record-writer");
    let watcher = daemon.control("parking-record-watcher");
    watcher.request(ControlRequest::Ping).unwrap();
    let ws = parking_workspace(&writer);
    let sessions = (0..3)
        .map(|_| parking_shell(&daemon, ws))
        .collect::<Vec<_>>();
    let (a, b, c) = (
        sessions[0].pane_id(),
        sessions[1].pane_id(),
        sessions[2].pane_id(),
    );
    writer
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(a),
            tab: None,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Horizontal,
            ratio: 0.4,
            new: seed(b),
            first: false,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: true,
            fallback: Some(a),
            restore: None,
        })
        .unwrap();
    let original = parking_tree(&writer, ws).tabs[0]
        .presentation
        .pip()
        .unwrap()
        .clone();
    writer
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: false,
            fallback: Some(a),
            restore: None,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Vertical,
            ratio: 0.6,
            new: seed(c),
            first: false,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneMove {
            workspace: ws,
            pane: b,
            to: c,
            axis: Axis::Vertical,
            first: false,
        })
        .unwrap();
    writer
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: None,
        })
        .unwrap();
    assert_ne!(
        parking_tree(&writer, ws).tabs[0].presentation.parked(),
        &[original.clone()]
    );
    writer
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: Some(original.clone()),
        })
        .unwrap();
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "metadata-only parking change was not broadcast"
        );
        if let Some(ControlEvent::Layout {
            workspace,
            delta: LayoutDelta::TabRestructured { tab, pane: None },
        }) = watcher.next_event(remaining)
        {
            if workspace == ws.to_string() && tab.presentation.parked() == [original.clone()] {
                break;
            }
        }
    }
    let persisted = MachineStore::open(daemon.dir.path().join(MACHINE_FILE)).machine();
    assert_eq!(
        persisted.workspaces[0].tabs[0].presentation.parked(),
        &[original.clone()]
    );
    let mismatch = writer
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: c,
            restore: Some(original.clone()),
        })
        .unwrap_err();
    assert_eq!(mismatch.kind(), std::io::ErrorKind::InvalidInput);
    let missing = writer
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: u64::MAX,
            restore: None,
        })
        .unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    writer.close();
    let reconnected = daemon.control("parking-record-reconnected");
    assert_eq!(
        parking_tree(&reconnected, ws).tabs[0].presentation.parked(),
        &[original]
    );
    reconnected
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: b,
            fallback: Some(c),
        })
        .unwrap();
    assert_eq!(
        parking_tree(&reconnected, ws).tabs[0]
            .visible_root()
            .unwrap()
            .pane_ids(),
        vec![a, b, c],
        "the supplied original relationship wins over a later move without removing the new split"
    );
    assert_eq!(
        panes
            .list()
            .unwrap()
            .iter()
            .filter(|pane| pane.alive)
            .count(),
        3
    );
    drop(sessions);
    for pane in [a, b, c] {
        panes.kill(pane).unwrap();
    }
}

#[test]
fn pip_geometry_follows_clients_without_replacing_the_live_terminal() {
    use lyrel_core::core::machine::PipGeometry;
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let writer = daemon.control("geometry-writer");
    let watcher = daemon.control("geometry-watcher");
    let ws = match writer
        .request(ControlRequest::WorkspaceCreate {
            name: Some("geometry".into()),
            workspace: None,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(ws) => ws.id,
        other => panic!("{other:?}"),
    };
    let mut shell = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    shell.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let pane = shell.pane_id();
    writer
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(pane),
            tab: None,
        })
        .unwrap();
    writer
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane,
            pip: true,
            fallback: None,
            restore: None,
        })
        .unwrap();
    let geometry = PipGeometry::new(0.1, 0.2, 0.65, 0.6).unwrap();
    writer
        .request(ControlRequest::PaneSetPipGeometry {
            workspace: ws,
            pane,
            geometry: Some(geometry),
        })
        .unwrap();
    let observe = |client: &ControlClient| {
        let ReplyOk::MachineTree(machine) = client.request(ControlRequest::MachineGet).unwrap()
        else {
            panic!("machine tree")
        };
        let tab = &machine.workspaces[0].tabs[0];
        assert_eq!(tab.presentation.pip().unwrap().pane(), pane);
        assert_eq!(tab.presentation.pip_geometry(), Some(geometry));
        assert!(tab.visible_root().is_none());
    };
    observe(&watcher);
    drop(writer);
    let reopened = daemon.control("geometry-reopened");
    observe(&reopened);
    shell
        .resize(WinSize {
            cols: 54,
            rows: 14,
            cell_w: 10,
            cell_h: 20,
        })
        .unwrap();
    #[cfg(unix)]
    {
        shell.input(b"stty size\r").unwrap();
        assert!(
            collect_until(&mut shell, b"14 54").1.is_none(),
            "the live PTY applies the new dimensions"
        );
    }
    shell.input(b"echo geometry_shell_survived\r").unwrap();
    assert!(
        collect_until(&mut shell, b"geometry_shell_survived")
            .1
            .is_none()
    );
    assert_eq!(
        panes
            .list()
            .unwrap()
            .iter()
            .filter(|entry| entry.alive)
            .count(),
        1
    );
    shell.detach().unwrap();
    let mut attached = panes.attach(pane, size()).unwrap();
    attached.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    assert!(
        collect_until(&mut attached, b"geometry_shell_survived")
            .1
            .is_none()
    );
    drop(attached);
    panes.kill(pane).unwrap();
}

mod kitty_graphics {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, Placement};

    mod fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/support/kitty_pty.rs"
        ));
    }
    use fixture::{hex, unhex};

    #[test]
    #[ignore = "PTY fixture launched explicitly by the daemon-session tests"]
    fn pty_sender() {
        fixture::pty_sender();
    }

    struct Consumer {
        session: lyrel_core::client::PaneSession,
        resources: BTreeMap<u64, Arc<Image>>,
        evicted: BTreeMap<u64, (u32, u32)>,
        placements: BTreeMap<u64, Placement>,
        serial: usize,
        updates: Vec<GraphicsUpdate>,
        last_output: Vec<u8>,
        screen: u64,
        history_origin: i64,
    }

    impl Consumer {
        fn spawn(daemon: &Daemon) -> Self {
            let shell = ShellSpec {
                program: std::env::current_exe().unwrap().to_str().unwrap().into(),
                args: vec![
                    "--ignored".into(),
                    "--exact".into(),
                    "kitty_graphics::pty_sender".into(),
                    "--nocapture".into(),
                    "--color=never".into(),
                    "--test-threads=1".into(),
                ],
                args_are_lyrel_defaults: false,
            };
            let mut session = daemon
                .panes()
                .spawn(None, size(), Some(shell), None, None)
                .unwrap();
            session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
            assert!(collect_until(&mut session, b"KITTY_READY").1.is_none());
            Self {
                session,
                resources: BTreeMap::new(),
                evicted: BTreeMap::new(),
                placements: BTreeMap::new(),
                serial: 0,
                updates: Vec::new(),
                last_output: Vec::new(),
                screen: 0,
                history_origin: 0,
            }
        }

        fn apply(&mut self, update: GraphicsUpdate) {
            match &update {
                GraphicsUpdate::Resource { key, image } => {
                    assert!(
                        !self.resources.contains_key(key),
                        "resource keys cannot be reused"
                    );
                    self.resources.insert(*key, image.clone());
                }
                GraphicsUpdate::Evicted { key, width, height } => {
                    self.resources.remove(key);
                    self.evicted.insert(*key, (*width, *height));
                }
                GraphicsUpdate::Place(placement) => {
                    assert!(
                        self.resources.contains_key(&placement.resource)
                            || self.evicted.contains_key(&placement.resource)
                    );
                    self.placements.insert(placement.key, placement.clone());
                }
                GraphicsUpdate::Screen {
                    screen,
                    history_origin,
                    ..
                } => {
                    self.screen = *screen;
                    self.history_origin = *history_origin;
                }
                GraphicsUpdate::Remove {
                    placements,
                    resources,
                } => {
                    for key in placements {
                        self.placements.remove(key);
                    }
                    for key in resources {
                        self.resources.remove(key);
                        self.evicted.remove(key);
                    }
                    assert!(
                        self.placements
                            .values()
                            .all(|p| self.resources.contains_key(&p.resource)
                                || self.evicted.contains_key(&p.resource))
                    );
                }
            }
            self.updates.push(update);
        }

        fn send(&mut self, bytes: &[u8], replies: usize) -> Vec<Vec<u8>> {
            self.exchange("SEND", &hex(bytes), replies, 0)
        }

        fn exchange(
            &mut self,
            operation: &str,
            payload: &str,
            replies: usize,
            fragment: usize,
        ) -> Vec<Vec<u8>> {
            self.serial += 1;
            self.updates.clear();
            let request = format!(
                "{operation} {} {replies} {payload} {fragment}\n",
                self.serial
            );
            self.session.input(request.as_bytes()).unwrap();
            let marker = format!("KITTY_DONE_{}\r", self.serial);
            let mut plain = Vec::new();
            loop {
                match self.session.recv().expect("ordered daemon graphics stream") {
                    DaemonMsg::Graphics(frame) => {
                        self.apply(
                            GraphicsUpdate::decode_frame_owned(frame.into_vec())
                                .expect("valid graphics update"),
                        );
                    }
                    DaemonMsg::Output(bytes) => {
                        assert!(
                            !windows_contain(&bytes, b"\x1b_G"),
                            "pixels must not enter text output"
                        );
                        plain.extend(bytes);
                        if windows_contain(&plain, marker.as_bytes()) {
                            break;
                        }
                    }
                    DaemonMsg::Exited { code } => panic!("PTY fixture exited: {code:?}"),
                    _ => {}
                }
            }
            self.last_output = plain;
            let prefix = format!("KITTY_REPLY {} ", self.serial);
            std::str::from_utf8(&self.last_output)
                .unwrap()
                .lines()
                .filter_map(|line| line.trim().strip_prefix(prefix.as_str()).map(unhex))
                .collect()
        }

        fn command(&mut self, control: &str) {
            assert!(
                self.send(format!("\x1b_G{control},q=2\x1b\\").as_bytes(), 0)
                    .is_empty()
            );
        }

        fn red(&mut self, control: &str) {
            self.send(
                format!("\x1b_G{control},f=32,s=1,v=1,q=2;/wAA/w==\x1b\\").as_bytes(),
                0,
            );
        }

        fn red_at(&mut self, row: u32, col: u32, control: &str) {
            self.send(
                format!("\x1b[{row};{col}H\x1b_G{control},f=32,s=1,v=1,q=2;/wAA/w==\x1b\\")
                    .as_bytes(),
                0,
            );
        }

        fn pixel(&self, placement: &Placement) -> Vec<u8> {
            self.resources[&placement.resource].to_rgba8().unwrap()
        }
    }

    #[test]
    fn daemon_graphics_unicode_prototypes_share_resources_but_ignore_physical_deletion() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-unicode");
        let mut pane = Consumer::spawn(&daemon);
        pane.red_at(4, 5, "a=T,i=7,p=2,U=1,c=4,r=2");
        let prototype = pane.placements.values().next().unwrap().clone();
        assert!(prototype.virtual_placement);
        pane.command("a=p,i=7,p=3,c=2,r=1,C=1");
        let direct = pane
            .placements
            .values()
            .find(|p| !p.virtual_placement)
            .unwrap()
            .clone();
        assert_eq!(prototype.resource, direct.resource);
        assert_eq!(
            (direct.anchor_row, direct.anchor_col),
            (3, 4),
            "U=1 does not move the cursor"
        );
        pane.command("a=d,d=A");
        assert_eq!(
            pane.placements.values().cloned().collect::<Vec<_>>(),
            [prototype.clone()]
        );
        assert_eq!(pane.pixel(&prototype), [255, 0, 0, 255]);
        pane.send(
            "\x1b[38;5;7m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes(),
            0,
        );
        pane.command("a=d,d=i,i=7,p=2");
        assert!(pane.placements.is_empty());
        pane.command("a=p,i=7,p=4,U=1,c=4,r=2");
        assert!(pane.placements.values().next().unwrap().virtual_placement);
        pane.command("a=d,d=I,i=7");
        assert!(pane.placements.is_empty());
        assert!(pane.resources.is_empty());
    }

    #[test]
    fn daemon_graphics_layout_user_clear_is_ordered_and_observers_cannot_mutate_it() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-clear");
        let mut pane = Consumer::spawn(&daemon);
        let id = pane.session.pane_id();
        let mut observer = daemon.panes().observe(id, size()).unwrap();
        observer.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
        pane.red_at(3, 2, "a=T,i=1,p=1,C=1");
        let original = pane.placements.values().next().unwrap().clone();
        observer.clear_presentation(true).unwrap();
        loop {
            if let DaemonMsg::Error(message) = observer.recv().unwrap() {
                assert!(message.contains("read-only"));
                break;
            }
        }
        pane.send(b"\x1b[0m", 0);
        assert_eq!(pane.placements[&original.key], original);
        pane.session.clear_presentation(true).unwrap();
        pane.send(b"\x1b[0m", 0);
        assert!(pane.placements.is_empty());
        assert!(
            pane.resources.is_empty(),
            "explicit history clearing releases active retained resources"
        );
        pane.send(b"\x1b[?1049h\x1b[?1049l", 0);
        assert!(
            pane.placements.is_empty(),
            "a later screen transition cannot revive cleared graphics"
        );
    }

    #[test]
    fn daemon_graphics_layout_reflows_the_text_anchor_and_updates_font_pixel_spans() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-reflow");
        let mut pane = Consumer::spawn(&daemon);
        pane.session
            .resize(WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            })
            .unwrap();
        let mut output = b"\x1b[2J\x1b[3J\x1b[H".to_vec();
        output.extend_from_slice(&[b'x'; 120]);
        output.extend_from_slice(
            b"\r\nanchor \x1b_Ga=T,i=1,p=1,f=32,s=1,v=1,c=2,r=2,C=1,q=2;/wAA/w==\x1b\\",
        );
        pane.send(&output, 0);
        let original = pane.placements.values().next().unwrap().clone();
        pane.session
            .resize(WinSize {
                cols: 40,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            })
            .unwrap();
        pane.send(b"\x1b[0m", 0);
        let reflowed = pane.placements[&original.key].clone();
        assert_eq!(
            reflowed.anchor_row,
            original.anchor_row + 1,
            "120 characters occupy three rather than two text rows after narrowing"
        );
        assert_eq!(reflowed.anchor_col, original.anchor_col);
        pane.session
            .resize(WinSize {
                cols: 40,
                rows: 24,
                cell_w: 16,
                cell_h: 32,
            })
            .unwrap();
        pane.send(b"\x1b[0m", 0);
        let scaled = &pane.placements[&original.key];
        assert_eq!(
            scaled.anchor_row, reflowed.anchor_row,
            "pixel-only changes must not reflow text"
        );
        assert_eq!((scaled.cols, scaled.rows), (2, 2));
        assert_eq!(pane.pixel(scaled), [255, 0, 0, 255]);
    }

    #[test]
    fn daemon_graphics_layout_resize_preserves_the_retained_slice() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-retained-resize");
        let mut pane = Consumer::spawn(&daemon);
        pane.session
            .resize(WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            })
            .unwrap();
        pane.send(b"\x1b[?1049h\x1b[2J", 0);
        pane.red_at(1, 1, "a=T,i=1,c=1,r=5,C=1");
        pane.send(b"\x1b[24;1H\n", 0);
        let clipped = pane.placements.values().next().unwrap().clone();
        assert_eq!((clipped.clip_top, clipped.clip_bottom), (1, 5));
        assert_eq!(clipped.anchor_row + 1 - pane.history_origin, 0);
        pane.session
            .resize(WinSize {
                cols: 40,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            })
            .unwrap();
        pane.send(b"\x1b[0m", 0);
        let resized = &pane.placements[&clipped.key];
        assert_eq!((resized.clip_top, resized.clip_bottom), (1, 5));
        assert_eq!(resized.anchor_row + 1 - pane.history_origin, 0);
        assert_eq!(pane.pixel(resized), [255, 0, 0, 255]);
        pane.send(b"\x1b[24;1H\n\n\n\n", 0);
        assert!(!pane.placements.contains_key(&clipped.key));
    }

    #[test]
    fn daemon_graphics_layout_anonymous_expiration_releases_resource_capacity() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-anonymous-expiration");
        let mut pane = Consumer::spawn(&daemon);
        pane.session
            .resize(WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            })
            .unwrap();
        pane.send(b"\x1b[?1049h\x1b[2J\x1b[H", 0);
        for _ in 0..16 {
            let upload = b"\x1b_Ga=T,f=32,s=1,v=1,C=1,q=2;/wAA/w==\x1b\\".repeat(64);
            pane.send(&upload, 0);
        }
        assert_eq!(pane.resources.len(), 1024);
        assert_eq!(pane.placements.len(), 1024);
        pane.send(b"\x1b[24;1H\n", 0);
        assert!(pane.placements.is_empty());
        assert!(
            pane.resources.is_empty(),
            "unaddressable anonymous pixels cannot consume capacity forever"
        );
        assert_eq!(
            pane.send(b"\x1b_Ga=T,i=2000,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\", 1),
            [b"\x1b_Gi=2000;OK\x1b\\".to_vec()]
        );
        let accepted = pane.placements.values().next().unwrap();
        assert_eq!(accepted.image_id, 2000);
        assert_eq!(pane.pixel(accepted), [255, 0, 0, 255]);
        // Named pixels remain addressable after losing their last placement.
        pane.send(b"\x1b[2J", 0);
        pane.command("a=p,i=2000,C=1");
        assert_eq!(pane.placements.values().next().unwrap().image_id, 2000);
    }

    #[test]
    fn daemon_graphics_layout_deccolm_resets_both_scroll_region_edges() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-deccolm");
        let mut pane = Consumer::spawn(&daemon);
        for mode in [b'h', b'l'] {
            for (operation, expected_row) in [
                ("\x1b[10;1H\n", 5),
                ("\x1b[3;1H\x1bM", 5),
                ("\x1b[S", 4),
                ("\x1b[T", 6),
            ] {
                pane.send(b"\x1b[?1049h\x1b[2J\x1b[3;10r", 0);
                pane.red_at(6, 2, "a=T,i=1,c=1,r=1,C=1");
                let key = pane
                    .placements
                    .values()
                    .find(|p| p.screen == 1)
                    .unwrap()
                    .key;
                pane.send(&[b'\x1b', b'[', b'?', b'3', mode], 0);
                pane.send(operation.as_bytes(), 0);
                let p = &pane.placements[&key];
                assert_eq!(
                    p.anchor_row - pane.history_origin,
                    expected_row,
                    "DECCOLM {mode} followed by {operation:?}"
                );
                assert_eq!((p.clip_top, p.clip_bottom), (0, 1));
                pane.send(b"\x1b[?1049l", 0);
            }
        }
    }

    #[test]
    fn daemon_graphics_layout_queries_follow_real_cell_metrics_once() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-queries");
        let mut pane = Consumer::spawn(&daemon);
        pane.session
            .resize(WinSize {
                cols: 63,
                rows: 19,
                cell_w: 15,
                cell_h: 30,
            })
            .unwrap();
        let replies = pane.send(b"\x1b[14t\x1b[16t", 2);
        assert_eq!(
            replies,
            [b"\x1b[4;570;945t".to_vec(), b"\x1b[6;30;15t".to_vec()]
        );
        pane.session
            .resize(WinSize {
                cols: 63,
                rows: 19,
                cell_w: 10,
                cell_h: 20,
            })
            .unwrap();
        assert_eq!(
            pane.send(b"\x1b[14t\x1b[16t", 2),
            [b"\x1b[4;380;630t".to_vec(), b"\x1b[6;20;10t".to_vec()]
        );
        // The next real Kitty query is a barrier as well as an assertion that
        // no duplicated geometry response remains in the child's input queue.
        assert_eq!(
            pane.send(b"\x1b_Ga=q,i=77,f=32,s=1,v=1;/wAA/w==\x1b\\", 1),
            [b"\x1b_Gi=77;OK\x1b\\".to_vec()]
        );
    }

    #[test]
    fn daemon_graphics_layout_scroll_regions_clip_only_contained_images() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-region");
        let mut pane = Consumer::spawn(&daemon);
        pane.send(b"\x1b[2J\x1b[H\x1b[3;10r", 0);
        pane.red_at(3, 2, "a=T,i=1,p=1,c=2,r=2,C=1");
        pane.red_at(2, 8, "a=T,i=2,p=2,c=2,r=3,C=1");
        let clipped = pane
            .placements
            .values()
            .find(|p| p.image_id == 1)
            .unwrap()
            .key;
        let crossing = pane
            .placements
            .values()
            .find(|p| p.image_id == 2)
            .unwrap()
            .clone();
        pane.send(b"\x1b[10;1H\n", 0);
        let p = &pane.placements[&clipped];
        assert_eq!(p.anchor_row - pane.history_origin, 1);
        assert_eq!((p.clip_top, p.clip_bottom), (1, 2));
        assert_eq!(
            pane.placements[&crossing.key], crossing,
            "a placement crossing a page margin stays fixed"
        );
        pane.send(b"\n", 0);
        assert!(
            !pane.placements.contains_key(&clipped),
            "fully clipped image cannot return"
        );
        assert!(pane.placements.contains_key(&crossing.key));
    }

    #[test]
    fn daemon_graphics_layout_buffers_clear_reset_and_expired_history() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-layout-screens");
        let mut pane = Consumer::spawn(&daemon);
        pane.red_at(3, 2, "a=T,i=1,p=1,C=1");
        let main = pane.placements.values().next().unwrap().clone();
        pane.send(b"\x1b[?1049h", 0);
        assert_eq!(pane.screen, 1);
        pane.red_at(3, 2, "a=T,i=1,p=1,C=1");
        let alt = pane
            .placements
            .values()
            .find(|p| p.screen == 1)
            .unwrap()
            .clone();
        assert_ne!(main.resource, alt.resource, "identities are screen-local");
        pane.send(b"\x1b[?1049l", 0);
        assert_eq!(pane.screen, 0);
        assert_eq!(pane.placements[&main.key], main);
        assert!(!pane.placements.contains_key(&alt.key));
        assert!(!pane.resources.contains_key(&alt.resource));
        pane.send(b"\x1b[?1049h\x1b[?1049l\x1b[2K\x1b[1J", 0);
        assert_eq!(
            pane.placements[&main.key], main,
            "ordinary text erasure does not delete graphics"
        );
        pane.send(b"\x1b[2J", 0);
        assert!(!pane.placements.contains_key(&main.key));
        pane.red_at(2, 1, "a=T,i=2,p=2,C=1");
        let historical = pane
            .placements
            .values()
            .find(|p| p.image_id == 2)
            .unwrap()
            .key;
        pane.send(b"\x1b[r\x1b[24;1H", 0);
        pane.send(&b"history row\r\n".repeat(10_100), 0);
        assert!(
            !pane.placements.contains_key(&historical),
            "expired text rows cannot drift onto live output"
        );
        pane.red_at(3, 2, "a=T,i=3,p=3,C=1");
        pane.send(b"\x1bc", 0);
        assert!(pane.placements.is_empty());
        assert!(pane.resources.is_empty());
    }

    #[test]
    fn daemon_graphics_refuses_resource_pressure_without_replacing_accepted_pixels() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-pressure");
        let mut pane = Consumer::spawn(&daemon);
        pane.red("a=T,i=1,p=1,C=1");
        let current = pane.placements.values().next().unwrap().clone();
        // Exercise the published 1,024-resource ceiling through the real PTY,
        // Protect every accepted resource with a visible placement. Merely
        // uploaded unreferenced images are now reclaimed before refusing data.
        for start in (2..=1024).step_by(64) {
            let mut upload = String::new();
            for id in start..=(start + 63).min(1024) {
                use std::fmt::Write as _;
                write!(
                    upload,
                    "\x1b_Ga=T,i={id},C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\"
                )
                .unwrap();
            }
            assert!(pane.send(upload.as_bytes(), 0).is_empty());
        }
        let refused = pane.send(b"\x1b_Ga=t,i=2000,f=32,s=1,v=1;/wAA/w==\x1b\\", 1);
        assert!(refused[0].starts_with(b"\x1b_Gi=2000;ENOSPC:"));
        assert_eq!(pane.placements[&current.key], current);
        assert_eq!(pane.pixel(&current), [255, 0, 0, 255]);
        let missing = pane.send(b"\x1b_Ga=p,i=2000,p=8,C=1\x1b\\", 1);
        assert!(missing[0].starts_with(b"\x1b_Gi=2000,p=8;ENOENT:"));
        // Explicit deletion releases that image and its visible placement.
        pane.command("a=d,d=I,i=2");
        let accepted = pane.send(b"\x1b_Ga=t,i=2000,f=32,s=1,v=1;/wAA/w==\x1b\\", 1);
        assert_eq!(accepted, [b"\x1b_Gi=2000;OK\x1b\\".to_vec()]);
        pane.command("a=p,i=2000,p=8,C=1");
        let newly_placed = pane
            .placements
            .values()
            .find(|p| p.image_id == 2000)
            .unwrap();
        assert_eq!(pane.pixel(newly_placed), [255, 0, 0, 255]);
        assert_eq!(pane.pixel(&current), [255, 0, 0, 255]);
    }

    #[test]
    fn daemon_graphics_transmit_place_identity_and_retransmission_lifecycle() {
        let daemon = Daemon::start();
        let control = daemon.control("graphics-lifecycle");
        assert_eq!(daemon.panes().version().unwrap().protocol, PROTOCOL_VERSION);
        let mut pane = Consumer::spawn(&daemon);
        let reply = pane.send(b"\x1b_Ga=t,i=11,f=32,s=2,v=1;/wAA/wD/AP8=\x1b\\", 1);
        assert_eq!(reply, [b"\x1b_Gi=11;OK\x1b\\".to_vec()]);
        assert!(pane.placements.is_empty(), "a=t only stores the resource");
        let resource = *pane.resources.keys().next().unwrap();
        assert_eq!(
            pane.resources[&resource].to_rgba8().unwrap(),
            [255, 0, 0, 255, 0, 255, 0, 255]
        );
        pane.command("a=p,i=11,p=8,x=1,y=0,w=99,h=1,c=3,r=2,X=1,Y=2,z=-2,C=1");
        let placed = pane.placements.values().next().unwrap().clone();
        assert_eq!(placed.resource, resource);
        assert_eq!((placed.image_id, placed.placement_id), (11, 8));
        assert_eq!((placed.x, placed.y, placed.w, placed.h), (1, 0, 1, 1));
        assert_eq!(
            (
                placed.cols,
                placed.rows,
                placed.offset_x,
                placed.offset_y,
                placed.z
            ),
            (3, 2, 1, 2, -2)
        );
        assert_eq!(&pane.pixel(&placed)[4..8], &[0, 255, 0, 255]);
        pane.command("a=p,i=11,p=8,c=2,r=1,C=1");
        assert_eq!(pane.placements.len(), 1);
        assert_eq!(
            pane.placements[&placed.key].cols, 2,
            "explicit p replaces only that placement"
        );
        pane.command("a=p,i=11,C=1");
        pane.command("a=p,i=11,C=1");
        assert_eq!(
            pane.placements.len(),
            3,
            "p=0 creates independent placements"
        );
        assert!(pane.placements.values().all(|p| p.resource == resource));
        pane.command("a=d,d=I,i=11,p=8");
        assert_eq!(pane.placements.len(), 2);
        assert!(
            pane.resources.contains_key(&resource),
            "other placements keep a scoped-deleted resource alive"
        );
        pane.red("a=t,i=11");
        assert!(
            pane.placements.is_empty(),
            "retransmission removes the old placements"
        );
        assert!(!pane.resources.contains_key(&resource));
        let replacement = *pane.resources.keys().next().unwrap();
        assert_ne!(replacement, resource);
        pane.command("a=p,i=11,C=1");
        assert_eq!(
            pane.pixel(pane.placements.values().next().unwrap()),
            [255, 0, 0, 255]
        );
        pane.red("a=T,C=1");
        pane.red("a=T,C=1");
        assert_eq!(
            pane.placements.len(),
            3,
            "anonymous images must not replace one another"
        );
        assert_eq!(pane.resources.len(), 3);
        pane.red("a=t,I=77");
        let older = *pane.resources.keys().max().unwrap();
        pane.command("a=p,I=77,p=4,C=1");
        let older_placement = pane
            .placements
            .values()
            .find(|p| p.resource == older)
            .unwrap()
            .clone();
        pane.red("a=t,I=77");
        let newest = *pane.resources.keys().max().unwrap();
        pane.command("a=p,I=77,p=4,C=1");
        let numbered = pane
            .placements
            .values()
            .find(|p| p.resource == newest)
            .unwrap();
        assert_ne!(numbered.image_id, 0);
        assert_ne!(
            numbered.image_id, older_placement.image_id,
            "repeated I allocates independent identities"
        );
        assert_ne!(numbered.resource, older_placement.resource);
        assert_eq!(pane.placements[&older_placement.key], older_placement);
        assert_eq!(pane.pixel(numbered), [255, 0, 0, 255]);
        // Control remains the same production dialect while pane v8 streams
        // authoritative updates; no testing API is exported by the daemon.
        assert!(matches!(
            control.request(ControlRequest::MachineGet).unwrap(),
            ReplyOk::MachineTree(_)
        ));
    }

    #[test]
    fn daemon_graphics_static_deletes_release_only_selected_lifetimes() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-delete");
        let mut pane = Consumer::spawn(&daemon);
        for target in ["i", "n", "c", "p", "q", "x", "y", "z", "r", "a"] {
            for release in [false, true] {
                pane.command("a=d,d=A");
                // Position and placement must share a request: the sender's
                // following text barrier deliberately moves the cursor.
                pane.red_at(2, 3, "a=T,I=81,p=1,c=2,r=2,z=-2,C=1");
                pane.red_at(6, 7, "a=T,I=82,p=2,c=2,r=2,z=4,C=1");
                let first = pane
                    .placements
                    .values()
                    .find(|p| p.placement_id == 1)
                    .unwrap()
                    .clone();
                let second = pane
                    .placements
                    .values()
                    .find(|p| p.placement_id == 2)
                    .unwrap()
                    .clone();
                let parameters = match target {
                    "i" => format!(",i={},p=1", first.image_id),
                    "n" => ",I=81".into(),
                    "p" => ",x=3,y=2".into(),
                    "q" => ",x=3,y=2,z=-2".into(),
                    "x" => ",x=3".into(),
                    "y" => ",y=2".into(),
                    "z" => ",z=-2".into(),
                    "r" => format!(",x={0},y={0}", first.image_id),
                    _ => String::new(),
                };
                let target_case = if release {
                    target.to_ascii_uppercase()
                } else {
                    target.into()
                };
                let selector = format!("d={target_case}{parameters}");
                pane.send(
                    format!("\x1b[2;3H\x1b_Ga=d,{selector},q=2\x1b\\").as_bytes(),
                    0,
                );
                assert!(!pane.placements.contains_key(&first.key), "{selector}");
                let all = target == "a";
                assert_eq!(
                    pane.placements.contains_key(&second.key),
                    !all,
                    "{selector}"
                );
                assert_eq!(
                    pane.resources.contains_key(&first.resource),
                    !release,
                    "{selector}"
                );
                assert_eq!(
                    pane.resources.contains_key(&second.resource),
                    !(release && all),
                    "{selector}"
                );
                if !all {
                    assert_eq!(pane.pixel(&second), [255, 0, 0, 255]);
                }
                if !release {
                    pane.command(&format!("a=p,i={},C=1", first.image_id));
                    assert!(
                        pane.placements
                            .values()
                            .any(|p| p.resource == first.resource)
                    );
                }
            }
        }
    }

    #[test]
    fn daemon_graphics_quiet_errors_are_correlated_and_panes_are_isolated() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-isolation");
        let mut first = Consumer::spawn(&daemon);
        let mut second = Consumer::spawn(&daemon);
        assert_ne!(first.session.pane_id(), second.session.pane_id());
        first.red("a=t,i=9");
        let missing = second.send(b"\x1b_Ga=p,i=9,p=7\x1b\\", 1);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].starts_with(b"\x1b_Gi=9,p=7;ENOENT"));
        assert!(second.resources.is_empty());
        assert!(second.placements.is_empty());
        second.send(b"\x1b_Ga=t,i=9,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\", 0);
        first.command("a=p,i=9,C=1");
        second.command("a=p,i=9,C=1");
        assert_eq!(
            first.pixel(first.placements.values().next().unwrap()),
            [255, 0, 0, 255]
        );
        assert_eq!(
            second.pixel(second.placements.values().next().unwrap()),
            [0, 255, 0, 255]
        );
        let quiet = first.send(
            b"\x1b_Ga=p,i=9,q=1,C=1\x1b\\\x1b_Ga=p,i=404,p=12,q=1\x1b\\",
            1,
        );
        assert_eq!(quiet.len(), 1, "q=1 suppresses success, not errors");
        assert!(quiet[0].starts_with(b"\x1b_Gi=404,p=12;ENOENT"));
        let silent = first.send(
            b"\x1b_Ga=p,i=405,p=13,q=2\x1b\\\x1b_Ga=p,i=406,p=14\x1b\\",
            1,
        );
        assert_eq!(silent.len(), 1, "q=2 suppresses errors as well");
        assert!(silent[0].starts_with(b"\x1b_Gi=406,p=14;ENOENT"));
        let malformed = first.send(b"\x1b_Ga=t,i=407,p=15,f=32,s=1,v=1;!\x1b\\", 1);
        assert_eq!(malformed.len(), 1);
        assert!(malformed[0].starts_with(b"\x1b_Gi=407,p=15;EINVAL"));
        let before = first.placements.clone();
        let crop = first.send(b"\x1b_Ga=p,i=9,p=16,x=99,w=1\x1b\\", 1);
        assert_eq!(crop.len(), 1);
        assert!(crop[0].starts_with(b"\x1b_Gi=9,p=16;EINVAL"));
        assert_eq!(
            first.placements, before,
            "an invalid crop cannot mutate accepted placements"
        );
        first.command("a=d,d=I,i=9");
        assert!(first.resources.is_empty());
        assert!(first.placements.is_empty());
        second.command("a=p,i=9,C=1");
        assert!(
            second
                .placements
                .values()
                .all(|p| second.pixel(p) == [0, 255, 0, 255])
        );
        // A fresh connection rebuilds its derived cache from daemon replay;
        // replaying the retained identity is not identity reuse in one stream.
        // Text snapshots must still exclude APC/base64 pixel payloads.
        let retained_key = *second.resources.keys().next().unwrap();
        let id = second.session.pane_id();
        second.session.detach().unwrap();
        second.resources.clear();
        second.evicted.clear();
        second.placements.clear();
        second.updates.clear();
        second.session = daemon.panes().attach(id, size()).unwrap();
        second
            .session
            .set_recv_timeout(Some(STREAM_WITHIN))
            .unwrap();
        loop {
            if let DaemonMsg::Snapshot(bytes) = second.session.recv().unwrap() {
                assert!(!windows_contain(&bytes, b"\x1b_G"));
                assert!(!windows_contain(&bytes, b"AP8A/w=="));
                break;
            }
        }
        second.command("a=p,i=9,C=1");
        assert_eq!(
            second.resources[&retained_key].to_rgba8().unwrap(),
            [0, 255, 0, 255],
            "without retransmission, replay restores the same pane-scoped resource identity and pixels"
        );
        assert!(
            second
                .placements
                .values()
                .all(|placement| placement.resource == retained_key)
        );
        assert!(
            second
                .placements
                .values()
                .all(|p| second.pixel(p) == [0, 255, 0, 255])
        );
    }

    #[test]
    fn daemon_graphics_cursor_effects_and_resize_share_the_ordered_output_stream() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-order");
        let mut pane = Consumer::spawn(&daemon);
        // A two-row placement moves down rows - 1 and right two columns.
        // The second must see that movement exactly once, before the sender
        // prints its barrier. No consumer-local selector calculation is used.
        pane.send(
            b"\x1b[3;4H\x1b_Ga=T,i=5,f=32,s=1,v=1,c=2,r=2,q=2;/wAA/w==\x1b\\\x1b_Ga=T,i=6,f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\\x1b_Ga=T,i=7,f=32,s=1,v=1,c=1,r=1,q=2;/wAA/w==\x1b\\\x1b_Ga=T,i=10,f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\",
            0,
        );
        let moved = pane
            .placements
            .values()
            .find(|p| p.image_id == 6)
            .unwrap()
            .clone();
        let stationary = pane
            .placements
            .values()
            .find(|p| p.image_id == 5)
            .unwrap()
            .clone();
        let same_row = pane
            .placements
            .values()
            .find(|p| p.image_id == 10)
            .unwrap()
            .clone();
        pane.command("a=d,d=p,x=6,y=4");
        assert!(
            !pane.placements.contains_key(&moved.key),
            "placement sees movement exactly once"
        );
        assert!(pane.placements.contains_key(&stationary.key));
        assert_eq!(pane.pixel(&stationary), [255, 0, 0, 255]);
        pane.command("a=d,d=p,x=7,y=4");
        assert!(
            !pane.placements.contains_key(&same_row.key),
            "a one-row placement moves right without moving down"
        );

        pane.session
            .resize(WinSize {
                cell_w: 1,
                cell_h: 1,
                ..size()
            })
            .unwrap();
        pane.send(
            b"\x1b[3;4H\x1b_Ga=T,i=8,f=32,s=2,v=1,C=1,q=2;/wAA/wD/AP8=\x1b\\",
            0,
        );
        let natural = pane
            .placements
            .values()
            .find(|p| p.image_id == 8)
            .unwrap()
            .clone();
        assert_eq!(
            (natural.cols, natural.rows),
            (0, 0),
            "wire retains natural-size request"
        );
        pane.command("a=d,d=p,x=5,y=3");
        assert!(
            !pane.placements.contains_key(&natural.key),
            "resized cell pixels determine occupied span"
        );
        assert_eq!(
            pane.resources[&natural.resource].to_rgba8().unwrap(),
            [255, 0, 0, 255, 0, 255, 0, 255]
        );

        let id = pane.session.pane_id();
        pane.session.detach().unwrap();
        let mut attached = daemon.panes().attach(id, size()).unwrap();
        attached.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
        loop {
            if let DaemonMsg::Snapshot(bytes) = attached.recv().unwrap() {
                assert!(
                    windows_contain(&bytes, b"\x1b[2C"),
                    "horizontal cursor effect is ordinary replay output"
                );
                assert!(
                    windows_contain(&bytes, b"\x1b[1B"),
                    "vertical cursor effect is ordinary replay output"
                );
                assert!(
                    !windows_contain(&bytes, b"\x1b[0B"),
                    "zero vertical movement must not become CSI 0 B"
                );
                assert!(!windows_contain(&bytes, b"/wAA/wD/AP8="));
                break;
            }
        }
    }

    #[test]
    fn daemon_graphics_streamed_formats_queries_and_recovery_keep_shell_usable() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-transfer");
        let mut pane = Consumer::spawn(&daemon);
        // Literal PNG: one straight-alpha red pixel, generated with a PNG
        // encoder, not by the decoder being exercised.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DQAAAEgQGALFXOsAAAAABJRU5ErkJggg==";
        let compressed_png = "eJzrDPBz5+WS4mJgYOD19HAJYmBgYARhDjYGBgZ50SOdYAkXx5CKOck/zh+4wMDA0sjYoBN6bgMDAwODp6ufyzqnhCYAzeMQYw==";
        for (index, format, compression, payload, expected) in [
            (0, 32, "", "/wAAgA==", vec![255, 0, 0, 128]),
            (1, 24, "", "/wAA", vec![255, 0, 0, 255]),
            (2, 32, ",o=z", "eJz7z8DQAAAEgAGA", vec![255, 0, 0, 128]),
            (3, 24, ",o=z", "eJz7z8AAAAMAAQA=", vec![255, 0, 0, 255]),
            (4, 100, "", png, vec![255, 0, 0, 128]),
            (5, 100, ",o=z", compressed_png, vec![255, 0, 0, 128]),
        ] {
            let id = 700 + index;
            let request = format!(
                "\x1b_Ga=T,i={id},p=4,f={format},s=1,v=1,C=1{compression},m=1;{}\x1b\\between_chunks\x1b_Gm=0;{}\x1b\\after_image",
                &payload[..4],
                &payload[4..]
            );
            assert_eq!(
                pane.exchange("SEND", &hex(request.as_bytes()), 1, 1),
                [format!("\x1b_Gi={id},p=4;OK\x1b\\").into_bytes()]
            );
            assert!(windows_contain(&pane.last_output, b"between_chunks"));
            assert!(windows_contain(&pane.last_output, b"after_image"));
            let placement = pane.placements.values().find(|p| p.image_id == id).unwrap();
            let image = &pane.resources[&placement.resource];
            assert_eq!((image.width, image.height), (1, 1));
            if format != 100 {
                assert_eq!(pane.pixel(placement), expected);
            } else {
                assert!(
                    !image.compressed,
                    "PNG compression is normalized before admission"
                );
                assert_eq!(&image.data[..8], b"\x89PNG\r\n\x1a\n");
            }
        }
        let retained = pane.placements.clone();
        for (header, payload, error) in [
            ("f=32,s=1,v=1", "!", "EINVAL"),
            ("f=32,s=1,v=1", "AAAA", "EINVAL"),
            ("f=32,s=1,v=1,o=z", "eJz7z8AAAAMAAQA=", "EINVAL"),
            (
                "f=32,s=1,v=1,o=z",
                "eJztwQENAAAAwqD3T20PBxQAAADQuQEQAAAB",
                "EINVAL",
            ),
            ("f=32,s=4294967295,v=4294967295", "AAAA", "ENOSPC"),
            ("f=100", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB", "EINVAL"),
            (
                "f=100",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DQAAAEgQGALVXOsAAAAABJRU5ErkJggg==",
                "EINVAL",
            ),
            (
                "f=100",
                "iVBORw0KGgoAAAANSUhEUgAAIAEAAAABCAYAAACZiUteAAAADUlEQVR4nGP4z8DQAAAEgQGALFXOsAAAAABJRU5ErkJggg==",
                "ENOSPC",
            ),
            ("f=100,o=z", "AAAA", "EINVAL"),
            ("f=1,s=1,v=1", "AAAA", "ENOTSUPPORTED"),
            ("f=32,t=x,s=1,v=1", "AAAA", "ENOTSUPPORTED"),
        ] {
            let reply = pane.send(
                format!("before\x1b_Ga=T,i=710,p=8,{header};{payload}\x1b\\after").as_bytes(),
                1,
            );
            assert!(reply[0].starts_with(format!("\x1b_Gi=710,p=8;{error}:").as_bytes()));
            assert_eq!(
                pane.placements, retained,
                "rejection preserves unrelated images"
            );
            assert!(windows_contain(&pane.last_output, b"beforeafter"));
        }
        let query = pane.send(
            b"\x1b_Ga=q,i=711,p=9,f=32,s=1,v=1,m=1;/wAA\x1b\\\x1b_Gm=0;gA==\x1b\\",
            1,
        );
        assert_eq!(query, [b"\x1b_Gi=711,p=9;OK\x1b\\".to_vec()]);
        assert_eq!(pane.placements, retained, "query never stores or displays");
        // Quiet and anonymous commands must not leave a reply queued in stdin:
        // the following SEND is parsed by the actual child, not a mock echo.
        pane.send(
            b"\x1b_Ga=q,q=2,i=712,f=32,s=1,v=1;!\x1b\\\x1b_Ga=t,f=32,s=1,v=1;/wAAgA==\x1b\\",
            0,
        );
        pane.send(
            b"\x1b_Ga=t,i=713,s=1,v=1,m=1;/wAA\x1b\\\x1b_Ga=d,d=I,i=713\x1b\\",
            0,
        );
        assert!(pane.placements.values().all(|p| p.image_id != 713));
        pane.red("a=T,i=714,C=1");
        assert_eq!(
            pane.pixel(
                pane.placements
                    .values()
                    .find(|p| p.image_id == 714)
                    .unwrap()
            ),
            [255, 0, 0, 255]
        );
        pane.send(b"subsequent_input_and_text_survive", 0);
        assert!(windows_contain(
            &pane.last_output,
            b"subsequent_input_and_text_survive"
        ));
        let oversized = pane.exchange("FLOOD", "00", 1, 0);
        assert!(oversized[0].starts_with(b"\x1b_Gi=720;ENOSPC:"));
        assert!(windows_contain(&pane.last_output, b"after_oversize"));
        pane.red("a=T,i=721,C=1");
        assert_eq!(
            pane.pixel(
                pane.placements
                    .values()
                    .find(|p| p.image_id == 721)
                    .unwrap()
            ),
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn daemon_graphics_capabilities_quiet_and_interrupted_control_are_truthful() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-capabilities");
        let mut pane = Consumer::spawn(&daemon);
        assert_eq!(
            pane.send(b"\x1b_Ga=q,i=730,p=3,f=24,s=1,v=1;AAAA\x1b\\", 1),
            [b"\x1b_Gi=730,p=3;OK\x1b\\".to_vec()]
        );
        assert!(pane.resources.is_empty(), "query cannot retain its fixture");
        for medium in ["f", "t", "s"] {
            let replies = pane.send(
                format!("\x1b_Ga=q,i=731,p=4,f=24,s=1,v=1,t={medium};L2RvZXMtbm90LWV4aXN0\x1b\\")
                    .as_bytes(),
                1,
            );
            let error = if cfg!(unix) { "EBADF" } else { "ENOTSUPPORTED" };
            assert!(
                replies[0].starts_with(format!("\x1b_Gi=731,p=4;{error}:").as_bytes()),
                "host-local capability must actually attempt to read the named object"
            );
        }
        for command in [
            "a=q,i=732,p=5,f=1,s=1,v=1;AAAA",
            "a=q,i=732,p=5,f=32,s=1,v=1,o=x;AAAA",
            "a=f,i=732,p=5;AAAA",
        ] {
            let reply = pane.send(format!("\x1b_G{command}\x1b\\").as_bytes(), 1);
            assert!(reply[0].starts_with(b"\x1b_Gi=732,p=5;ENOTSUPPORTED:"));
        }
        let invalid_ids = pane.send(b"\x1b_Ga=q,i=733,I=8,p=6,f=24,s=1,v=1;AAAA\x1b\\", 1);
        assert!(invalid_ids[0].starts_with(b"\x1b_Gi=733,I=8,p=6;EINVAL:"));
        // Every silent request is followed by another command read by the PTY
        // child; unsolicited bytes would corrupt that next SEND and fail it.
        for command in [
            "a=q,f=24,s=1,v=1;AAAA",
            "a=q,i=734,q=1,f=24,s=1,v=1;AAAA",
            "a=q,i=734,q=2,f=24,s=1,v=1;!",
            "a=q,i=734,I=8,q=2,f=24,s=1,v=1;AAAA",
        ] {
            assert!(
                pane.send(format!("\x1b_G{command}\x1b\\").as_bytes(), 0)
                    .is_empty()
            );
        }
        for interruption in ["\x18", "\x1a", "\x1b[31m", "\x1b_Ga=d,d=i,i=735\x1b\\"] {
            let request = format!(
                "\x1b_Ga=T,i=735,s=1,v=1,m=1;/wAA\x1b\\\x1b_Gm=0;bad{interruption}\x1b_Ga=T,i=736,s=1,v=1,C=1,q=2;/wAAgA==\x1b\\recovered"
            );
            let replies = pane.send(request.as_bytes(), 1);
            assert!(replies[0].starts_with(b"\x1b_Gi=735;EINVAL:"));
            assert!(pane.placements.values().all(|p| p.image_id != 735));
            assert_eq!(
                pane.pixel(
                    pane.placements
                        .values()
                        .find(|p| p.image_id == 736)
                        .unwrap()
                ),
                [255, 0, 0, 128]
            );
            assert!(windows_contain(&pane.last_output, b"recovered"));
        }
        pane.send(b"\x1b_Ga=T,i=737,s=1,v=1,m=1;/wAA\x1b\\between\x1b_Ga=d,d=i,i=736\x1b\\\x1b_Gm=0;gA==\x1b\\after_delete", 0);
        assert!(
            pane.placements
                .values()
                .all(|p| p.image_id != 736 && p.image_id != 737)
        );
        assert!(windows_contain(&pane.last_output, b"between"));
        assert!(windows_contain(&pane.last_output, b"after_delete"));
        pane.red("a=T,i=738,C=1");
        assert_eq!(
            pane.pixel(
                pane.placements
                    .values()
                    .find(|p| p.image_id == 738)
                    .unwrap()
            ),
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn daemon_graphics_continuation_quiet_is_independent_of_key_order() {
        let daemon = Daemon::start();
        let _control = daemon.control("graphics-continuation-quiet");
        let mut pane = Consumer::spawn(&daemon);
        for (initial_quiet, continuation, payload, replies) in [
            (0, "q=1,m=0", "gA==", 0),
            (0, "q=2,m=0", "!", 0),
            (0, "m=0,q=2", "!", 0),
            (0, "q=1,m=0", "!", 1),
            (2, "q=0,m=0", "!", 1),
        ] {
            let request = format!(
                "\x1b_Ga=q,i=739,p=7,s=1,v=1,q={initial_quiet},m=1;/wAA\x1b\\\x1b_G{continuation};{payload}\x1b\\after_continuation"
            );
            let actual = pane.send(request.as_bytes(), replies);
            assert_eq!(actual.len(), replies);
            if replies != 0 {
                assert!(actual[0].starts_with(b"\x1b_Gi=739,p=7;EINVAL:"));
            }
            assert!(
                pane.resources.is_empty(),
                "queries must not retain resources"
            );
            assert!(windows_contain(&pane.last_output, b"after_continuation"));
        }
        pane.send(b"input_after_quiet_continuation", 0);
        assert!(windows_contain(
            &pane.last_output,
            b"input_after_quiet_continuation"
        ));
    }
}
