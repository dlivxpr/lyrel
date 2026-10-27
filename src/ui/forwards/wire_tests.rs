//! Stateful protocol peer: mutations take effect independently of their replies.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::TestAppContext;

use super::route::ForwardOwnerKey;
use crate::daemon::protocol::{
    ClientMsg, DaemonMsg, ForwardStatus, ManagedForward, SshForwardKind, WorkspaceOp,
};
use crate::terminal::PaneWorkspace;
use crate::ui::app::test_window::harness_with_tabs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mutation {
    Add,
    Remove(u64),
}

struct Registry {
    owner: ForwardOwnerKey,
    rules: Vec<ManagedForward>,
    next_id: u64,
    mutations: Vec<Mutation>,
    lose: Option<(Mutation, bool)>,
    reject_add: bool,
}

impl Registry {
    fn request(&mut self, request: ClientMsg) -> Option<DaemonMsg> {
        let op = match request {
            ClientMsg::OnWorkspace(request) => {
                assert_eq!(self.owner, ForwardOwnerKey::Workspace(request.workspace));
                assert_eq!(request.view_pane, 1);
                request.op
            }
            ClientMsg::ListForwards { pane_id } => {
                assert_eq!(self.owner, ForwardOwnerKey::Pane(pane_id));
                WorkspaceOp::ListForwards
            }
            ClientMsg::AddForward { pane_id, rule } => {
                assert_eq!(self.owner, ForwardOwnerKey::Pane(pane_id));
                WorkspaceOp::AddForward { rule }
            }
            ClientMsg::RemoveForward { pane_id, forward_id } => {
                assert_eq!(self.owner, ForwardOwnerKey::Pane(pane_id));
                WorkspaceOp::RemoveForward { forward_id }
            }
            _ => return Some(DaemonMsg::Error("not a forwarding request".into())),
        };
        let mutation = match &op {
            WorkspaceOp::AddForward { .. } => Some(Mutation::Add),
            WorkspaceOp::RemoveForward { forward_id } => Some(Mutation::Remove(*forward_id)),
            WorkspaceOp::ListForwards => None,
            _ => panic!("unexpected forwarding operation: {op:?}"),
        };
        let lost = mutation.and_then(|mutation| {
            self.mutations.push(mutation);
            if self.lose.as_ref().is_some_and(|(kind, _)| *kind == mutation) {
                self.lose.take().map(|(_, apply)| apply)
            } else {
                None
            }
        });
        if lost != Some(false) {
            match op {
                WorkspaceOp::AddForward { rule } => {
                    let status = if std::mem::take(&mut self.reject_add) {
                        ForwardStatus::Error("bind refused".into())
                    } else {
                        ForwardStatus::Listening
                    };
                    self.rules.push(ManagedForward {
                        id: self.next_id,
                        pane_id: 1,
                        kind: rule.kind,
                        bind_host: rule.bind_host,
                        bind_port: if rule.bind_port == 0 { 49152 } else { rule.bind_port },
                        target_host: rule.target_host,
                        target_port: rule.target_port,
                        description: rule.description,
                        enabled: rule.enabled,
                        status,
                    });
                    self.next_id += 1;
                }
                WorkspaceOp::RemoveForward { forward_id } => {
                    self.rules.retain(|rule| rule.id != forward_id);
                }
                WorkspaceOp::ListForwards => {}
                _ => unreachable!(),
            }
        }
        lost.is_none().then(|| DaemonMsg::ForwardList(self.rules.clone()))
    }
}

struct WirePeer {
    registry: Arc<Mutex<Registry>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WirePeer {
    fn start() -> Self {
        let listener = crate::daemon::transport::bind().unwrap();
        listener.set_nonblocking(true).unwrap();
        let registry = Arc::new(Mutex::new(Registry {
            owner: ForwardOwnerKey::Pane(1),
            rules: Vec::new(),
            next_id: 10,
            mutations: Vec::new(),
            lose: None,
            reject_add: false,
        }));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state = registry.clone();
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                        crate::daemon::transport::authenticate(&mut stream).unwrap();
                        let request = ClientMsg::read(&mut stream).unwrap();
                        if let Some(reply) = state.lock().unwrap().request(request) {
                            reply.encode(&mut stream).unwrap();
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("accept wire request: {error}"),
                }
            }
        });
        Self { registry, stop, thread: Some(thread) }
    }
}

impl Drop for WirePeer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
        crate::daemon::transport::remove_stale_endpoint();
    }
}

fn original() -> ManagedForward {
    ManagedForward {
        id: 1,
        pane_id: 1,
        kind: SshForwardKind::Local,
        bind_host: "127.0.0.1".into(),
        bind_port: 8080,
        target_host: "10.0.0.5".into(),
        target_port: 80,
        description: None,
        enabled: true,
        status: ForwardStatus::Listening,
    }
}

fn workspace() -> PaneWorkspace {
    let spec = serde_json::from_str::<crate::daemon::protocol::NativeSshSpec>(
        r#"{"host":"wire-fixture","port":22,"user":"me","auth_mode":"auto"}"#,
    ).unwrap();
    PaneWorkspace {
        workspace: crate::core::session::WorkspaceId::new(),
        target: crate::core::session::RemoteTarget::direct(&spec.user, &spec.host, spec.port),
        spec: Some(Box::new(spec)),
        label: None,
        resize_echo: false,
    }
}

// Each invocation owns its endpoint/config in a child process. Binding a socket
// under the ordinary GPUI scratch dir would change unrelated disconnected tests.
#[gpui_kit::test]
fn managed_forward_wire_recovery(cx: &mut TestAppContext) {
    const CHILD_CONFIG: &str = "LYREL_FORWARD_WIRE_TEST_CONFIG";
    let Some(config) = std::env::var_os(CHILD_CONFIG) else {
        let dir = tempfile::TempDir::new().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", concat!(module_path!(), "::managed_forward_wire_recovery")
                .split_once("::").unwrap().1, "--nocapture", "--test-threads=1"])
            .env(CHILD_CONFIG, dir.path())
            .output().unwrap();
        assert!(output.status.success(), "wire recovery child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    };
    crate::core::config::set_config_dir(config.into());
    let peer = WirePeer::start();
    for use_workspace in [false, true] {
        for (mutation, applied) in [
            (Mutation::Add, true),
            (Mutation::Add, false),
            (Mutation::Remove(1), true),
            (Mutation::Remove(1), false),
        ] {
            let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
            let workspace = use_workspace.then(workspace);
            {
                let mut state = peer.registry.lock().unwrap();
                state.owner = workspace.as_ref().map_or(ForwardOwnerKey::Pane(1), |workspace| {
                    ForwardOwnerKey::Workspace(workspace.workspace)
                });
                state.rules = vec![original()];
                state.mutations.clear();
                state.lose = Some((mutation, applied));
            }
            app.update_in(&mut vcx, |app, window, cx| {
                app.tabs[0].pane.terminals()[0].update(cx, |view, _| {
                    view.set_workspace(workspace.clone());
                });
                app.refresh_managed_forwards(1, cx);
                app.loopback_panel.edit(original(), window, cx);
                app.loopback_panel.mf_target_host.update(cx, |input, cx| {
                    input.set_value("10.0.0.6", window, cx)
                });
                app.add_managed_forward(1, window, cx);
                assert!(app.loopback_panel.is_open_for(1));
                assert!(app.loopback_panel.mf_error.is_some());
                assert_eq!(app.loopback_panel.mf_target_host.read(cx).value().as_str(), "10.0.0.6");
                let expected = if mutation == Mutation::Add {
                    vec![Mutation::Remove(1), Mutation::Add]
                } else {
                    vec![Mutation::Remove(1)]
                };
                assert_eq!(peer.registry.lock().unwrap().mutations, expected,
                    "an unanswered mutation must not trigger retry or rollback");
                app.add_managed_forward(1, window, cx);
                assert!(!app.loopback_panel.is_open_for(1));
                let state = peer.registry.lock().unwrap();
                assert_eq!(state.rules.len(), 1, "reconciliation must not duplicate the rule");
                assert_eq!(state.rules[0].target_host, "10.0.0.6");
                assert_eq!(app.loopback_panel.managed(), state.rules.as_slice());
                assert_eq!(state.mutations.iter().filter(|m| **m == Mutation::Add).count(),
                    if mutation == Mutation::Add && !applied { 2 } else { 1 });
            });
        }

        for applied in [false, true] {
            let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
            let workspace = use_workspace.then(workspace);
            {
                let mut state = peer.registry.lock().unwrap();
                state.owner = workspace.as_ref().map_or(ForwardOwnerKey::Pane(1), |workspace| {
                    ForwardOwnerKey::Workspace(workspace.workspace)
                });
                state.rules.clear();
                state.mutations.clear();
                let failed_id = state.next_id;
                state.lose = Some((Mutation::Remove(failed_id), applied));
                state.reject_add = true;
            }
            app.update_in(&mut vcx, |app, window, cx| {
                app.tabs[0].pane.terminals()[0].update(cx, |view, _| {
                    view.set_workspace(workspace.clone());
                });
                app.toggle_managed_forward_form(1, window, cx);
                app.loopback_panel.mf_target_port.update(cx, |input, cx| {
                    input.set_value("3000", window, cx)
                });
                app.add_managed_forward(1, window, cx);
                assert!(app.loopback_panel.is_open_for(1));
                assert_eq!(peer.registry.lock().unwrap().mutations.len(), 2,
                    "unanswered rejected-rule cleanup cannot trigger alternate-port add");
                assert_eq!(app.loopback_panel.mf_target_port.read(cx).value().as_str(), "3000");
                app.add_managed_forward(1, window, cx);
                assert!(!app.loopback_panel.is_open_for(1));
                let state = peer.registry.lock().unwrap();
                assert_eq!(state.rules.len(), 1);
                assert_eq!(state.rules[0].bind_port, 3000);
                assert!(matches!(state.rules[0].status, ForwardStatus::Listening));
            });
        }
    }
}
