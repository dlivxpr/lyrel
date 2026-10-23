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
    let ws = match writer.request(ControlRequest::WorkspaceCreate {
        name: Some("pip".into()), workspace: None,
    }).unwrap() {
        ReplyOk::WorkspaceTree(ws) => ws.id,
        other => panic!("{other:?}"),
    };
    let mut session = panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let original = session.pane_id();
    writer.request(ControlRequest::TabCreate {
        workspace: ws, at: None, pane: seed(original), tab: None,
    }).unwrap();
    let get = |client: &ControlClient| -> Machine {
        match client.request(ControlRequest::MachineGet).unwrap() {
            ReplyOk::MachineTree(machine) => *machine,
            other => panic!("{other:?}"),
        }
    };
    let present = |pane, pip, fallback| {
        writer.request(ControlRequest::PaneSetPresentation { workspace: ws, pane, pip, fallback, restore: None }).unwrap();
    };
    let initial = get(&writer);
    session.input(if cfg!(windows) { b"set lyrel_pip_state=retained\r" } else { b"lyrel_pip_state=retained\r" }).unwrap();
    present(original, true, None);
    let single = get(&watcher);
    let tab = &single.workspaces[0].tabs[0];
    assert_eq!(tab.root, initial.workspaces[0].tabs[0].root);
    assert_eq!(tab.presentation.pip().unwrap().pane(), original);
    assert_eq!(tab.visible_root(), None, "single-pane PiP does not create a filler shell");
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let event = watcher.next_event(deadline.saturating_duration_since(Instant::now())).expect("PiP delta");
        if matches!(event, ControlEvent::Layout { delta: LayoutDelta::TabRestructured { tab, pane: None }, .. }
            if tab.presentation.pip().is_some_and(|pip| pip.pane() == original)) {
            break;
        }
        assert!(Instant::now() < deadline, "no presentation delta");
    }
    assert_eq!(panes.list().unwrap().iter().filter(|p| p.alive).count(), 1);
    assert!(writer.request(ControlRequest::PaneSetPresentation {
        workspace: ws, pane: u64::MAX, pip: true, fallback: None,
        restore: None,
    }).is_err());
    assert_eq!(get(&writer).workspaces[0].tabs[0].presentation, tab.presentation);
    present(original, false, None);
    assert_eq!(get(&writer).workspaces[0].tabs[0], initial.workspaces[0].tabs[0],
        "ordinary restore only unhides and keeps the complete logical tree");
    present(original, true, None);
    panes.send_input(original, if cfg!(windows) {
        b"echo PIP_%lyrel_pip_state%\r"
    } else {
        b"printf 'PIP_%s\\n' \"$lyrel_pip_state\"\r"
    }).unwrap();
    assert!(collect_until(&mut session, b"PIP_retained").1.is_none());
    session.detach().unwrap();
    let mut captured = panes.attach(original, size()).unwrap();
    captured.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    assert!(collect_until(&mut captured, b"PIP_retained").1.is_none(), "capture/replay retains the original session");

    let sibling_session = panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap();
    let sibling = sibling_session.pane_id();
    writer.request(ControlRequest::PaneSplit {
        workspace: ws, pane: original, axis: Axis::Vertical, ratio: 0.3,
        new: seed(sibling), first: false,
    }).unwrap();
    present(sibling, true, Some(original));
    assert_eq!(get(&writer).workspaces[0].tabs[0].presentation.pip().unwrap().pane(), sibling);
    present(original, true, Some(sibling));
    let replacement_session = panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap();
    let replacement = replacement_session.pane_id();
    writer.request(ControlRequest::PaneReplace {
        workspace: ws, old: original, new: seed(replacement),
    }).unwrap();
    assert_eq!(get(&writer).workspaces[0].tabs[0].presentation.pip().unwrap().pane(), replacement);
    let original_relationship = get(&writer).workspaces[0].tabs[0].presentation.pip().cloned();

    let fallback_session = panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap();
    let fallback = fallback_session.pane_id();
    writer.request(ControlRequest::PaneSplit {
        workspace: ws, pane: sibling, axis: Axis::Horizontal, ratio: 0.6,
        new: seed(fallback), first: false,
    }).unwrap();
    let other_session = panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap();
    let other = other_session.pane_id();
    writer.request(ControlRequest::PaneSplit {
        workspace: ws, pane: fallback, axis: Axis::Horizontal, ratio: 0.6,
        new: seed(other), first: false,
    }).unwrap();
    writer.request(ControlRequest::PaneClose { workspace: ws, pane: sibling }).unwrap();
    writer.request(ControlRequest::PaneMove {
        workspace: ws, pane: replacement, to: other, axis: Axis::Horizontal, first: false,
    }).unwrap();
    writer.request(ControlRequest::PaneSetPresentation {
        workspace: ws, pane: replacement, pip: true, fallback: Some(fallback),
        restore: original_relationship.clone(),
    }).unwrap();
    assert_eq!(get(&writer).workspaces[0].tabs[0].presentation.pip(), original_relationship.as_ref(),
        "same-pane synchronization restores original relationship after arbitrary reshaping");
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let event = watcher.next_event(deadline.saturating_duration_since(Instant::now())).expect("metadata-only delta");
        if matches!(event, ControlEvent::Layout { delta: LayoutDelta::TabRestructured { tab, pane: None }, .. }
            if tab.root.pane_ids() == vec![fallback, other, replacement]
                && tab.presentation.pip() == original_relationship.as_ref()) {
            break;
        }
        assert!(Instant::now() < deadline, "same-pane metadata change was not broadcast");
    }
    let persisted_restore = lyrel_core::core::machine::MachineStore::open(daemon.dir.path().join(
        lyrel_core::core::machine::MACHINE_FILE)).machine();
    assert_eq!(persisted_restore.workspaces[0].tabs[0].presentation.pip(), original_relationship.as_ref());
    let mismatch = writer.request(ControlRequest::PaneSetPresentation {
        workspace: ws, pane: other, pip: true, fallback: None, restore: original_relationship.clone(),
    }).unwrap_err();
    assert_eq!(mismatch.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(get(&writer).workspaces[0].tabs[0].presentation.pip(), original_relationship.as_ref());
    let before_restore = panes.list().unwrap();
    present(replacement, false, Some(fallback));
    let restored = get(&writer);
    let tab = &restored.workspaces[0].tabs[0];
    assert!(tab.presentation.pip().is_none());
    assert_eq!(tab.root, PaneNode::Split {
        axis: Axis::Horizontal, ratio: 0.6,
        a: Box::new(PaneNode::Split {
            axis: Axis::Vertical, ratio: 0.3,
            a: Box::new(PaneNode::Leaf { pane: replacement }),
            b: Box::new(PaneNode::Leaf { pane: fallback }),
        }),
        b: Box::new(PaneNode::Leaf { pane: other }),
    }, "only the restored pane moves next to the chosen current fallback");
    assert_eq!(panes.list().unwrap().iter().map(|p| (p.pane_id, p.alive)).collect::<Vec<_>>(),
        before_restore.iter().map(|p| (p.pane_id, p.alive)).collect::<Vec<_>>(),
        "presentation changes never close or spawn a shell");
    present(replacement, true, Some(fallback));
    writer.close();
    let reconnected = daemon.control("pip-reconnected");
    let durable = get(&reconnected);
    assert_eq!(durable.workspaces[0].tabs[0].presentation.pip().unwrap().pane(), replacement);
    let persisted = lyrel_core::core::machine::MachineStore::open(daemon.dir.path().join(
        lyrel_core::core::machine::MACHINE_FILE)).machine();
    assert_eq!(persisted.workspaces[0].tabs[0].presentation, durable.workspaces[0].tabs[0].presentation);
    reconnected.request(ControlRequest::PaneMove {
        workspace: ws, pane: replacement, to: other, axis: Axis::Horizontal, first: false,
    }).unwrap();
    let moved = get(&reconnected);
    assert_eq!(moved.workspaces[0].tabs[0].presentation.pip().unwrap().pane(), replacement);
    assert_eq!(moved.workspaces[0].tabs[0].root.pane_ids(), vec![fallback, other, replacement]);
    assert_eq!(moved.workspaces[0].tabs[0].visible_root().unwrap().pane_ids(), vec![fallback, other]);
    reconnected.request(ControlRequest::PaneSetPresentation {
        workspace: ws, pane: other, pip: false, fallback: Some(fallback),
        restore: None,
    }).unwrap();
    assert_eq!(get(&reconnected).workspaces[0].tabs[0].presentation, moved.workspaces[0].tabs[0].presentation,
        "a stale restore cannot dismiss another pane's PiP");
    reconnected.request(ControlRequest::PaneClose { workspace: ws, pane: replacement }).unwrap();
    let closed = get(&reconnected);
    assert!(closed.workspaces[0].tabs[0].presentation.pip().is_none());
    assert!(!closed.workspaces[0].tabs[0].root.contains(replacement));
    assert_eq!(closed.workspaces[0].tabs[0].root.pane_ids(), vec![fallback, other]);
    panes.send_input(original, if cfg!(windows) { b"echo STILL_%lyrel_pip_state%\r" } else { b"printf 'STILL_%s\\n' \"$lyrel_pip_state\"\r" }).unwrap();
    assert!(collect_until(&mut captured, b"STILL_retained").1.is_none(), "replacing tree identity never replaces the old live PTY implicitly");
    drop((captured, sibling_session, replacement_session, fallback_session, other_session));
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
    let ws = match control.request(ControlRequest::WorkspaceCreate { name: None, workspace: None }).unwrap() {
        ReplyOk::WorkspaceTree(ws) => ws.id,
        other => panic!("{other:?}"),
    };
    let sessions = (0..3).map(|_| panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap()).collect::<Vec<_>>();
    let (a, b, c) = (sessions[0].pane_id(), sessions[1].pane_id(), sessions[2].pane_id());
    control.request(ControlRequest::TabCreate { workspace: ws, at: None, pane: seed(a), tab: None }).unwrap();
    control.request(ControlRequest::PaneSplit { workspace: ws, pane: a, axis: Axis::Horizontal, ratio: 0.4, new: seed(b), first: false }).unwrap();
    control.request(ControlRequest::PaneSetPresentation { workspace: ws, pane: b, pip: true, fallback: Some(a), restore: None }).unwrap();
    control.request(ControlRequest::PaneSplit { workspace: ws, pane: a, axis: Axis::Vertical, ratio: 0.6, new: seed(c), first: false }).unwrap();
    control.request(ControlRequest::PaneMove { workspace: ws, pane: b, to: c, axis: Axis::Vertical, first: false }).unwrap();
    control.request(ControlRequest::PaneSetPresentation { workspace: ws, pane: b, pip: false, fallback: Some(c), restore: None }).unwrap();
    let tree = match control.request(ControlRequest::WorkspaceTree { workspace: ws }).unwrap() {
        ReplyOk::WorkspaceTree(ws) => ws,
        other => panic!("{other:?}"),
    };
    let tab = &tree.tabs[0];
    assert!(tab.presentation.pip().is_none());
    assert_eq!(tab.root.pane_ids(), vec![a, b, c]);
    assert!(matches!(&tab.root, PaneNode::Split { axis: Axis::Vertical, a: restored, b: untouched, .. }
        if matches!(&**restored, PaneNode::Split { axis: Axis::Horizontal, a: original, b: returned, .. }
            if original.contains(a) && returned.contains(b)) && untouched.contains(c)),
        "restore rejoins the surviving original sibling without replaying its old subtree over the newer split");
    assert_eq!(panes.list().unwrap().iter().filter(|pane| pane.alive).count(), 3);
    drop(sessions);
    for pane in [a, b, c] { panes.kill(pane).unwrap(); }
}

#[test]
fn pip_geometry_follows_clients_without_replacing_the_live_terminal() {
    use lyrel_core::core::machine::PipGeometry;
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let writer = daemon.control("geometry-writer");
    let watcher = daemon.control("geometry-watcher");
    let ws = match writer.request(ControlRequest::WorkspaceCreate {
        name: Some("geometry".into()), workspace: None,
    }).unwrap() {
        ReplyOk::WorkspaceTree(ws) => ws.id,
        other => panic!("{other:?}"),
    };
    let mut shell = panes.spawn(None, size(), Some(interactive_shell()), None, None).unwrap();
    shell.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let pane = shell.pane_id();
    writer.request(ControlRequest::TabCreate { workspace: ws, at: None, pane: seed(pane), tab: None }).unwrap();
    writer.request(ControlRequest::PaneSetPresentation { workspace: ws, pane, pip: true, fallback: None, restore: None }).unwrap();
    let geometry = PipGeometry::new(0.1, 0.2, 0.65, 0.6).unwrap();
    writer.request(ControlRequest::PaneSetPipGeometry { workspace: ws, pane, geometry: Some(geometry) }).unwrap();
    let observe = |client: &ControlClient| {
        let ReplyOk::MachineTree(machine) = client.request(ControlRequest::MachineGet).unwrap() else { panic!("machine tree") };
        let tab = &machine.workspaces[0].tabs[0];
        assert_eq!(tab.presentation.pip().unwrap().pane(), pane);
        assert_eq!(tab.presentation.pip_geometry(), Some(geometry));
        assert!(tab.visible_root().is_none());
    };
    observe(&watcher);
    drop(writer);
    let reopened = daemon.control("geometry-reopened");
    observe(&reopened);
    shell.resize(WinSize { cols: 54, rows: 14, cell_w: 10, cell_h: 20 }).unwrap();
    #[cfg(unix)]
    {
        shell.input(b"stty size\r").unwrap();
        assert!(collect_until(&mut shell, b"14 54").1.is_none(), "the live PTY applies the new dimensions");
    }
    shell.input(b"echo geometry_shell_survived\r").unwrap();
    assert!(collect_until(&mut shell, b"geometry_shell_survived").1.is_none());
    assert_eq!(panes.list().unwrap().iter().filter(|entry| entry.alive).count(), 1);
    shell.detach().unwrap();
    let mut attached = panes.attach(pane, size()).unwrap();
    attached.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    assert!(collect_until(&mut attached, b"geometry_shell_survived").1.is_none());
    drop(attached);
    panes.kill(pane).unwrap();
}
