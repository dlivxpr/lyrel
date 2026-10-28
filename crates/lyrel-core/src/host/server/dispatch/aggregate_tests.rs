use super::*;
use super::super::{AttachRegistry, MachineStore, PaneDirectory, Services, serve_with};
use crate::daemon::control::PaneAgentState;
use crate::daemon::protocol::PaneInfo;
use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use crate::daemon::control::ControlClient;
use crate::daemon::control::{ControlHello, feature};
use crate::host::local::LocalHost;
use std::net::{TcpListener, TcpStream};

struct ThreePanesOneAgent {
    panes: Vec<PaneInfo>,
}

impl PaneDirectory for ThreePanesOneAgent {
    fn pane_count(&self) -> u64 {
        3
    }

    fn panes(&self) -> Vec<PaneInfo> {
        self.panes.clone()
    }

    fn pane_procs(&self, pane_id: u64) -> crate::daemon::protocol::PaneProcs {
        crate::daemon::protocol::PaneProcs {
            procs: vec![crate::daemon::protocol::ProcEntry {
                pid: 900 + pane_id as u32,
                name: "node".into(),
                depth: 0,
                foreground: true,
                ..Default::default()
            }],
            ports: vec![crate::daemon::protocol::PortEntry {
                port: 3000,
                pid: 900 + pane_id as u32,
                name: "node".into(),
                addr: "*".into(),
            }],
            probe: Default::default(),
            context: None,
        }
    }

    fn hibernate_pane(&self, _pane_id: u64) {}

    fn close_pane(&self, _pane_id: u64) {}

    fn agent_states(&self) -> Vec<PaneAgentState> {
        vec![PaneAgentState {
            pane_id: 7,
            agent: Some(CLIAgent::OhMyPi),
            state: AgentSessionState {
                status: AgentStatus::Working,
                session_id: Some("sess-7".into()),
                ..Default::default()
            },
        }]
    }
}

fn client_with(services: Services) -> ControlClient {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _ = serve_with(stream, LocalHost::new(), services);
    });
    let sock = TcpStream::connect(addr).unwrap();
    ControlClient::over_tcp(
        sock,
        &ControlHello::host_rpc("tok", "test-host"),
        Box::new(|_| {}),
    )
    .unwrap()
}

#[test]
fn status_answers_with_this_servers_facts() {
    let services = Services {
        panes: Some(Arc::new(ThreePanesOneAgent { panes: Vec::new() })),
        ..Services::none()
    };
    let client = client_with(services);

    let ReplyOk::Status(status) = client.call(ControlRequest::Status).unwrap() else {
        panic!("Status must answer with ReplyOk::Status");
    };
    assert_eq!(status.pid, std::process::id());
    assert_eq!(status.panes, 3);
    assert_eq!(status.control_version, CONTROL_VERSION);
    assert_eq!(
        status.protocol_version,
        crate::daemon::protocol::PROTOCOL_VERSION
    );
    assert_eq!(status.build, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        status.socket,
        control_endpoint_display(),
        "the CLI dials whatever path Status names"
    );
    assert!(status.uptime_secs <= server_started().elapsed().as_secs());
}

#[test]
fn agent_states_are_the_pane_directorys_snapshot() {
    let services = Services {
        panes: Some(Arc::new(ThreePanesOneAgent { panes: Vec::new() })),
        ..Services::none()
    };
    let client = client_with(services);

    let ReplyOk::AgentStates(states) = client.call(ControlRequest::AgentStates).unwrap() else {
        panic!("AgentStates must answer with ReplyOk::AgentStates");
    };
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].pane_id, 7);
    assert_eq!(states[0].agent, Some(CLIAgent::OhMyPi));
    assert_eq!(states[0].state.status, AgentStatus::Working);
    assert_eq!(states[0].state.session_id.as_deref(), Some("sess-7"));
}

/// A remote workspace's pane runs on the peer, so the peer is the only
/// one that can walk its process tree — the client's own daemon has never
/// heard of the pane. Without this request its ports were simply invisible.
#[test]
fn a_peer_says_what_is_listening_inside_one_of_its_panes() {
    let services = Services {
        panes: Some(Arc::new(ThreePanesOneAgent { panes: Vec::new() })),
        ..Services::none()
    };
    let client = client_with(services);

    assert!(
        client.hello().has_feature(feature::PANE_PROCS),
        "a peer that serves panes has to say it can describe them"
    );
    let ReplyOk::PaneProcs(procs) = client
        .call(ControlRequest::PaneProcs { pane_id: 4 })
        .unwrap()
    else {
        panic!("PaneProcs must answer with ReplyOk::PaneProcs");
    };
    assert_eq!(procs.ports.len(), 1);
    assert_eq!(procs.ports[0].port, 3000);
    assert_eq!(
        procs.ports[0].pid, 904,
        "the answer is about the pane that was asked for"
    );
}

/// The feature is the client's only way to tell "nothing is listening"
/// from "nobody here can tell you", and a process serving no panes is the
/// second. Announcing it anyway would draw an empty Ports list over a
/// question that was never answered.
#[test]
fn a_process_with_no_panes_does_not_claim_it_can_list_ports() {
    let client = client_with(Services::none());
    assert!(!client.hello().has_feature(feature::PANE_PROCS));
}

#[test]
fn machine_get_overlays_live_pane_titles() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(
            ws.id,
            None,
            machine::PaneSeed {
                pane: 7,
                cwd: Some("/repo/lyrel".into()),
                ssh_spec: None,
                agent: None,
                shell: None,
            },
            None,
            None,
        )
        .unwrap();

    let services = Services {
        machine: Some(store),
        attachments: Arc::new(AttachRegistry::default()),
        panes: Some(Arc::new(ThreePanesOneAgent {
            panes: vec![PaneInfo {
                pane_id: 7,
                cwd: Some(PathBuf::from("/repo/lyrel")),
                title: "nvim".into(),
                osc_title: Some("nvim ~/repo/lyrel".into()),
                alive: true,
                owner: None,
            }],
        })),
    };
    let client = client_with(services);
    let ReplyOk::MachineTree(machine) = client.call(ControlRequest::MachineGet).unwrap() else {
        panic!("MachineGet must answer with a machine tree");
    };
    let pane = machine.panes.iter().find(|p| p.id == 7).unwrap();
    assert_eq!(pane.title, "nvim");
    assert_eq!(pane.osc_title.as_deref(), Some("nvim ~/repo/lyrel"));
    assert!(pane.live);
}

#[test]
fn aggregates_still_answer_when_this_process_serves_no_panes() {
    let client = client_with(Services::none());

    let ReplyOk::AgentStates(states) = client.call(ControlRequest::AgentStates).unwrap() else {
        panic!("AgentStates must answer with ReplyOk::AgentStates");
    };
    assert!(states.is_empty());

    let ReplyOk::Status(status) = client.call(ControlRequest::Status).unwrap() else {
        panic!("Status must answer with ReplyOk::Status");
    };
    assert_eq!(status.panes, 0);

    let ReplyOk::Routes(routes) = client.call(ControlRequest::Routes).unwrap() else {
        panic!("Routes must answer with ReplyOk::Routes");
    };
    assert!(
        routes.iter().all(|r| !r.key.is_empty()),
        "whatever links exist are named; none are blank"
    );
}

/// Records which panes were stopped for a sleeping tab, and in what order
/// relative to the tree: each entry carries whether the tree already
/// called the tab asleep when the pane was stopped.
struct SleepRecorder {
    store: Arc<MachineStore>,
    stopped: std::sync::Mutex<Vec<(u64, bool)>>,
}

impl PaneDirectory for SleepRecorder {
    fn pane_count(&self) -> u64 {
        0
    }

    fn panes(&self) -> Vec<PaneInfo> {
        Vec::new()
    }

    fn pane_procs(&self, _pane_id: u64) -> crate::daemon::protocol::PaneProcs {
        Default::default()
    }

    fn agent_states(&self) -> Vec<PaneAgentState> {
        Vec::new()
    }

    fn hibernate_pane(&self, pane_id: u64) {
        let asleep = self.store.machine().workspaces.iter().any(|w| {
            w.tabs
                .iter()
                .any(|t| t.hibernated && t.root.contains(pane_id))
        });
        self.stopped.lock().unwrap().push((pane_id, asleep));
    }

    fn close_pane(&self, _pane_id: u64) {}
}

fn seed(pane: u64) -> machine::PaneSeed {
    machine::PaneSeed {
        pane,
        cwd: Some("/repo".into()),
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

/// Going to sleep stops every pane of the tab — after the tree says the
/// tab is asleep, so nobody reading the tree in between takes the stopped
/// panes for dead ones — and keeps them in the tree for the wake. Waking
/// only clears the mark: the client brings the panes back.
#[test]
fn a_tab_put_to_sleep_stops_its_panes_and_keeps_them_in_the_tree() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    let tab = store.tab_create(ws.id, None, seed(7), None, None).unwrap();
    store
        .pane_split(
            ws.id,
            7,
            machine::Axis::Horizontal,
            0.5,
            seed(8),
            false,
            None,
        )
        .unwrap();
    let other = store.tab_create(ws.id, None, seed(9), None, None).unwrap();
    let recorder = Arc::new(SleepRecorder {
        store: Arc::clone(&store),
        stopped: std::sync::Mutex::new(Vec::new()),
    });
    let client = client_with(Services {
        machine: Some(Arc::clone(&store)),
        attachments: Arc::new(AttachRegistry::default()),
        panes: Some(recorder.clone()),
    });
    assert!(client.hello().has_feature(feature::TAB_HIBERNATE));

    let ReplyOk::Panes(panes) = client
        .call(ControlRequest::TabSetHibernated {
            workspace: ws.id,
            tab: tab.id,
            hibernated: true,
        })
        .unwrap()
    else {
        panic!("TabSetHibernated must answer with the tab's panes");
    };
    assert_eq!(panes, vec![7, 8]);
    assert_eq!(
        *recorder.stopped.lock().unwrap(),
        vec![(7, true), (8, true)],
        "both panes stopped, each after the tree marked the tab"
    );
    let machine = store.machine();
    let asleep = &machine.workspaces[0].tabs;
    assert!(asleep.iter().find(|t| t.id == tab.id).unwrap().hibernated);
    assert!(!asleep.iter().find(|t| t.id == other.id).unwrap().hibernated);
    for pane in [7, 8] {
        assert!(
            machine.panes.iter().any(|p| p.id == pane),
            "pane {pane}'s record is what the wake restores from"
        );
    }

    recorder.stopped.lock().unwrap().clear();
    let ReplyOk::Panes(_) = client
        .call(ControlRequest::TabSetHibernated {
            workspace: ws.id,
            tab: tab.id,
            hibernated: false,
        })
        .unwrap()
    else {
        panic!("TabSetHibernated must answer with the tab's panes");
    };
    assert!(recorder.stopped.lock().unwrap().is_empty());
    assert!(!store.machine().workspaces[0].tabs[0].hibernated);
}

/// A peer with a tree and no panes could mark a tab asleep and leave its
/// shells running. It does not offer to, and refuses if asked anyway —
/// before the mark, so the tree is not left claiming a sleep that never
/// happened.
#[test]
fn a_peer_serving_no_panes_cannot_put_a_tab_to_sleep() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    let tab = store.tab_create(ws.id, None, seed(7), None, None).unwrap();
    let client = client_with(Services::with_machine(Arc::clone(&store)));
    assert!(client.hello().has_feature(feature::MACHINE_TREE));
    assert!(!client.hello().has_feature(feature::TAB_HIBERNATE));
    assert!(
        client
            .call(ControlRequest::TabSetHibernated {
                workspace: ws.id,
                tab: tab.id,
                hibernated: true,
            })
            .is_err()
    );
    assert!(!store.machine().workspaces[0].tabs[0].hibernated);
}

/// Records what a remembered close does to the panes: each stopped pane
/// with whether the tree already listed it as a closed tab's at the time,
/// and each pane ended for good.
struct CloseRecorder {
    store: Arc<MachineStore>,
    stopped: std::sync::Mutex<Vec<(u64, bool)>>,
    ended: std::sync::Mutex<Vec<u64>>,
}

impl PaneDirectory for CloseRecorder {
    fn pane_count(&self) -> u64 {
        0
    }

    fn panes(&self) -> Vec<PaneInfo> {
        Vec::new()
    }

    fn pane_procs(&self, _pane_id: u64) -> crate::daemon::protocol::PaneProcs {
        Default::default()
    }

    fn agent_states(&self) -> Vec<PaneAgentState> {
        Vec::new()
    }

    fn hibernate_pane(&self, pane_id: u64) {
        let remembered = self
            .store
            .machine()
            .workspaces
            .iter()
            .any(|w| w.closed.iter().any(|c| c.tab.root.contains(pane_id)));
        self.stopped.lock().unwrap().push((pane_id, remembered));
    }

    fn close_pane(&self, pane_id: u64) {
        self.ended.lock().unwrap().push(pane_id);
    }
}

/// Closing to remember stops the tab's panes keeping their screens — after
/// the entry is in the tree, so the sweeps never see them as nobody's —
/// ends the pane the tree never had, and reopening answers the tab with the
/// records it is rebuilt from, once.
#[test]
fn a_remembered_close_stops_the_panes_and_a_reopen_hands_the_tab_back() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    let tab = store.tab_create(ws.id, None, seed(7), None, None).unwrap();
    store
        .pane_split(ws.id, 7, machine::Axis::Vertical, 0.5, seed(8), false, None)
        .unwrap();
    let recorder = Arc::new(CloseRecorder {
        store: Arc::clone(&store),
        stopped: std::sync::Mutex::new(Vec::new()),
        ended: std::sync::Mutex::new(Vec::new()),
    });
    let client = client_with(Services {
        machine: Some(Arc::clone(&store)),
        attachments: Arc::new(AttachRegistry::default()),
        panes: Some(recorder.clone()),
    });
    assert!(client.hello().has_feature(feature::CLOSED_TABS));

    let ReplyOk::Panes(stopped) = client
        .call(ControlRequest::TabCloseRemembered {
            workspace: ws.id,
            tab: tab.id,
            panes: vec![7, 8, 11],
        })
        .unwrap()
    else {
        panic!("TabCloseRemembered must answer with the panes it stopped");
    };
    assert_eq!(stopped, vec![7, 8]);
    assert_eq!(
        *recorder.stopped.lock().unwrap(),
        vec![(7, true), (8, true)],
        "both stopped, each after the tree listed the tab as closed"
    );
    assert_eq!(*recorder.ended.lock().unwrap(), vec![11]);

    let ReplyOk::ReopenedTab(Some(reopened)) = client
        .call(ControlRequest::TabReopen {
            workspace: ws.id,
            tab: None,
        })
        .unwrap()
    else {
        panic!("TabReopen must answer with the closed tab");
    };
    assert_eq!(reopened.tab.id, tab.id);
    assert_eq!(reopened.tab.root.pane_ids(), vec![7, 8]);
    let cwds: Vec<_> = reopened.panes.iter().map(|p| p.cwd.as_deref()).collect();
    assert_eq!(cwds, vec![Some("/repo"), Some("/repo")]);

    let again = client
        .call(ControlRequest::TabReopen {
            workspace: ws.id,
            tab: None,
        })
        .unwrap();
    assert_eq!(again, ReplyOk::ReopenedTab(None));
}

/// Without panes there is nothing to keep a screen with, so the peer does
/// not offer the list — a client then closes and reopens the old way — and
/// refuses the close if asked anyway, leaving the tab where it was.
#[test]
fn a_peer_serving_no_panes_does_not_offer_closed_tabs() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    let tab = store.tab_create(ws.id, None, seed(7), None, None).unwrap();
    let client = client_with(Services::with_machine(Arc::clone(&store)));
    assert!(!client.hello().has_feature(feature::CLOSED_TABS));
    assert!(
        client
            .call(ControlRequest::TabCloseRemembered {
                workspace: ws.id,
                tab: tab.id,
                panes: vec![7],
            })
            .is_err()
    );
    let machine = store.machine();
    assert_eq!(machine.workspaces[0].tabs.len(), 1);
    assert!(machine.workspaces[0].closed.is_empty());
}
