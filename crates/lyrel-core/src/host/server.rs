use std::io;
#[cfg(all(test, unix))]
use std::io::Read;
use std::sync::Arc;
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
use requests::Requests;
use subscriptions::{GitStreams, Watches};

#[cfg(unix)]
use unix::control_endpoint_display;
#[cfg(windows)]
use windows::control_endpoint_display;

pub const MAX_WORKERS: usize = 64;

pub const WORKER_LINGER: Duration = Duration::from_secs(10);

pub const MAX_QUEUED: usize = 1024;

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
    pool: Pool,
    machine: Option<Arc<MachineStore>>,
    machine_origin: Option<machine::SubscriberId>,
    identity: ConnectionIdentity,
    panes: Option<Arc<dyn PaneDirectory>>,
}

impl Conn {
    fn machine(&self) -> io::Result<&Arc<MachineStore>> {
        self.machine
            .as_ref()
            .ok_or_else(|| io::Error::other("this server does not serve the machine tree"))
    }
}

pub const CONTROL_SOCK_ENV: &str = "LYREL_CONTROL_SOCK";

#[cfg(all(unix, test))]
pub(crate) use unix::socket_path_in;
#[cfg(unix)]
pub use unix::{
    bind_control_socket, control_endpoint_answers, control_socket_path, serve_listener,
    serve_listener_with, spawn_control_listener, spawn_control_listener_with,
};

#[cfg(windows)]
pub use windows::{
    CONTROL_PORT_FILE, connect_control, control_endpoint_answers, control_endpoint_path,
    remove_control_endpoint, spawn_control_listener, spawn_control_listener_with,
};
