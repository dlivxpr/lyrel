use std::collections::HashSet;
use std::io;
#[cfg(all(test, unix))]
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::core::machine::{self, MachineStore};
#[cfg(all(test, unix))]
use crate::daemon::control::{
    ControlClientMsg, ControlEvent, ControlReply, ControlRequest, ControlServerMsg, ReplyOk,
};
use crate::daemon::control::{PaneAgentState, server_started};
use crate::daemon::protocol::PaneInfo;
use crate::host::SharedHost;

mod attachments;
mod connection;
mod dispatch;
mod negotiation;
mod output;
mod pool;
mod read_sessions;
mod requests;
mod subscriptions;
#[cfg(all(test, unix))]
mod test_support;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub use attachments::AttachRegistry;
use attachments::ConnectionIdentity;
pub use connection::{serve, serve_halves, serve_halves_with, serve_with};
use output::Sink;
use pool::Pool;
use read_sessions::ReadSessions;
use requests::Requests;
use subscriptions::{GitStreams, Watches};

#[cfg(unix)]
use unix::control_endpoint_display;
#[cfg(windows)]
use windows::control_endpoint_display;

pub const MAX_WORKERS: usize = 64;

pub const WORKER_LINGER: Duration = Duration::from_secs(10);

pub const MAX_QUEUED: usize = 1024;

/// Read views one control connection may hold. A client opens one per open
/// database document; the bound keeps a peer from pinning transactions on the
/// server indefinitely.
pub const MAX_SQLITE_VIEWS: usize = 8;

pub const LAYOUT_EVENT_QUEUE: usize = 1024;

pub trait PaneDirectory: Send + Sync {
    fn pane_count(&self) -> u64;
    fn panes(&self) -> Vec<PaneInfo>;
    fn agent_states(&self) -> Vec<PaneAgentState>;
    /// What is running inside one pane, and what it is listening on.
    ///
    /// Answered by whoever owns the pane's PTY, which for a remote workspace
    /// is this process and not the client's own daemon: the client asks over
    /// the control link precisely because the processes are here.
    fn pane_procs(&self, pane_id: u64) -> crate::daemon::protocol::PaneProcs;
    /// The verified scope ending one of the pane's processes would cover —
    /// the #372 confirmation path. `Err` carries the daemon's reason the
    /// identity could not be vouched for; unlike `pane_procs`, a refusal
    /// here is information, not absence. Directories that serve no local
    /// panes refuse: their far side is exactly what this request exists to
    /// avoid acting on.
    fn pane_end_process_plan(
        &self,
        _pane_id: u64,
        _pid: u32,
        _started: Option<u64>,
    ) -> Result<crate::daemon::protocol::EndProcessPlan, String> {
        Err("this directory does not serve process termination".to_string())
    }
    /// Terminate exactly the confirmed identities and report one outcome per
    /// identity, including the ones refused as drifted. The default refuses
    /// every identity, mirroring `pane_end_process_plan`.
    fn pane_end_process(
        &self,
        _pane_id: u64,
        scope: &[crate::daemon::protocol::ProcIdentity],
    ) -> Vec<crate::daemon::protocol::EndProcessOutcome> {
        scope
            .iter()
            .map(|id| crate::daemon::protocol::EndProcessOutcome {
                pid: id.pid,
                name: String::new(),
                state: crate::daemon::protocol::EndProcessState::NotOwned,
            })
            .collect()
    }
    /// Stop a pane for a tab going to sleep: the processes go, the pane's last
    /// screen stays on disk for the wake to open with. Unlike closing a pane,
    /// which drops that screen because nobody will ask for it again.
    fn hibernate_pane(&self, pane_id: u64);
    /// End a pane for good: the processes go and so does its stored screen,
    /// because nothing will ask to restore it again — a pane the tree never
    /// recorded, or one whose closed tab has aged off the recently-closed list.
    fn close_pane(&self, pane_id: u64);
}

#[derive(Clone, Default)]
pub struct Services {
    pub machine: Option<Arc<MachineStore>>,
    pub attachments: Arc<AttachRegistry>,
    pub panes: Option<Arc<dyn PaneDirectory>>,
}

impl Services {
    pub fn none() -> Services {
        Services::default()
    }

    pub fn with_machine(store: Arc<MachineStore>) -> Services {
        Services {
            machine: Some(store),
            attachments: Arc::new(AttachRegistry::default()),
            panes: None,
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn control_endpoint_display() -> String {
    String::new()
}

// One negotiated connection owns all request and subscription state. Each private
// owner keeps its transitions local without adding locks or output authorities.
struct Conn {
    host: SharedHost,
    sink: Arc<Sink>,
    requests: Requests,
    watches: Watches,
    git_streams: GitStreams,
    read_sessions: ReadSessions,
    /// SQLite read views this connection opened. A view belongs to the
    /// connection that opened it: another peer's id is not addressable, and
    /// every one of these is closed when the link ends.
    sqlite_views: Mutex<HashSet<u64>>,
    pool: Pool,
    machine: Option<Arc<MachineStore>>,
    machine_origin: Option<machine::SubscriberId>,
    identity: ConnectionIdentity,
    panes: Option<Arc<dyn PaneDirectory>>,
}

impl Conn {
    /// Release every SQLite view this connection opened. Called on every exit
    /// path: a disconnect must not leave a read transaction holding a WAL.
    fn close_sqlite_views(&self) {
        let views = {
            let mut views = self.sqlite_views.lock().unwrap_or_else(|e| e.into_inner());
            views.drain().collect::<Vec<_>>()
        };
        for view in views {
            self.host.sqlite_close(view);
        }
    }

    fn machine(&self) -> io::Result<&Arc<MachineStore>> {
        self.machine
            .as_ref()
            .ok_or_else(|| io::Error::other("this server does not serve the machine tree"))
    }
}

/// Overrides the control socket path. Wire-side discovery lives in
/// `lyrel-protocol`; re-exported here so existing
/// `lyrel_core::host::server::*` paths keep working.
#[cfg(unix)]
pub use lyrel_protocol::daemon::control_endpoint::CONTROL_SOCK_ENV;

#[cfg(all(unix, test))]
pub(crate) use lyrel_protocol::daemon::control_endpoint::socket_path_in;
#[cfg(unix)]
pub use lyrel_protocol::daemon::control_endpoint::{
    connect_control, control_endpoint_answers, control_socket_path, remove_control_endpoint,
};
#[cfg(unix)]
pub use unix::{
    bind_control_socket, serve_listener, serve_listener_with, spawn_control_listener,
    spawn_control_listener_with,
};

#[cfg(windows)]
pub use lyrel_protocol::daemon::control_endpoint::{
    CONTROL_PORT_FILE, connect_control, control_endpoint_answers, control_endpoint_path,
    remove_control_endpoint,
};
#[cfg(windows)]
pub use windows::{spawn_control_listener, spawn_control_listener_with};
