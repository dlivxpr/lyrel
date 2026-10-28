use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
#[cfg(all(test, unix))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::core::machine::{self, MachineStore};
use crate::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlEvent, ControlReply, ControlRequest, ControlServerMsg,
    GIT_STREAM_CHUNK, GIT_STREAM_CHUNK_MAX, MAX_CONCURRENT_GIT_STREAMS, PaneAgentState, ReplyOk,
    ServerStatus, WATCH_BURST_CAP, WireError, WireErrorKind, server_started,
};
use crate::daemon::protocol::PaneInfo;
use crate::host::{Host, SearchHit, SharedHost, WatchSub};

mod attachments;
mod connection;
mod negotiation;
mod output;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(all(test, unix))]
mod test_support;

pub use attachments::AttachRegistry;
use attachments::ConnectionIdentity;
pub use connection::{serve, serve_halves, serve_halves_with, serve_with};
use output::Sink;

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


fn read_loop<R: Read>(r: &mut R, conn: &Arc<Conn>) -> io::Result<()> {
    loop {
        let msg = ControlClientMsg::read(r)?;
        match msg {
            ControlClientMsg::Hello(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control peer sent a second HELLO on an established connection",
                ));
            }
            ControlClientMsg::Request { req_id, req } => submit(conn, req_id, req, Vec::new()),
            ControlClientMsg::RequestBlob { req_id, req, blob } => submit(conn, req_id, req, blob),
            ControlClientMsg::Cancel { req_id } => conn.cancel(req_id),
        }
    }
}

fn submit(conn: &Arc<Conn>, req_id: u64, req: ControlRequest, blob: Vec<u8>) {
    conn.inflight
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(req_id, false);

    let job_conn = Arc::clone(conn);
    let queued = conn
        .pool
        .submit(move || run_job(&job_conn, req_id, req, blob));
    if !queued {
        conn.finish(
            req_id,
            ControlReply::Err(WireError::new(
                WireErrorKind::Other,
                "the control server has more requests queued than it will hold",
            )),
            Vec::new(),
            false,
        );
    }
}

fn run_job(conn: &Arc<Conn>, req_id: u64, req: ControlRequest, blob: Vec<u8>) {
    if conn.is_cancelled(req_id) {
        conn.forget(req_id);
        return;
    }

    let wants_blob = req.returns_blob();
    let (reply, out_blob) = match run_request(conn, req_id, req, blob) {
        Ok((ok, bytes)) => (ControlReply::Ok(ok), bytes),
        Err(e) => (ControlReply::Err(WireError::from_io(&e)), Vec::new()),
    };
    conn.finish(req_id, reply, out_blob, wants_blob);
}

fn drop_unsendable_hits(hits: &mut Vec<SearchHit>) {
    let before = hits.len();
    hits.retain(|hit| hit.path.to_str().is_some());
    if hits.len() != before {
        log::debug!(
            "search dropped {} hit(s) whose paths are not UTF-8",
            before - hits.len()
        );
    }
}

/// [`drop_unsendable_hits`] for content hits: a path that is not UTF-8 cannot
/// cross the JSON wire intact, and a hit the client cannot open is worse than
/// one it never sees.
fn drop_unsendable_content_hits(hits: &mut Vec<crate::host::ContentHit>) {
    let before = hits.len();
    hits.retain(|hit| hit.path.to_str().is_some());
    if hits.len() != before {
        log::debug!(
            "content search dropped {} hit(s) whose paths are not UTF-8",
            before - hits.len()
        );
    }
}

fn machine_with_live_panes(conn: &Conn) -> io::Result<machine::Machine> {
    let mut machine = conn.machine()?.machine();
    let Some(panes) = conn.panes.as_ref().map(|p| p.panes()) else {
        return Ok(machine);
    };
    for info in panes {
        let record = match machine.panes.iter_mut().find(|p| p.id == info.pane_id) {
            Some(record) => record,
            None => {
                machine.panes.push(machine::PaneRecord::new(info.pane_id));
                machine.panes.last_mut().expect("record was just inserted")
            }
        };
        record.cwd = info.cwd.map(|p| p.to_string_lossy().into_owned());
        record.title = info.title;
        record.osc_title = info.osc_title;
        record.live = info.alive;
    }
    Ok(machine)
}

fn run_request(
    conn: &Arc<Conn>,
    req_id: u64,
    req: ControlRequest,
    blob: Vec<u8>,
) -> io::Result<(ReplyOk, Vec<u8>)> {
    let h: &dyn Host = &*conn.host;
    let p = |s: &str| PathBuf::from(s);

    Ok(match req {
        ControlRequest::Ping => (ReplyOk::Pong, Vec::new()),
        ControlRequest::HostMetrics => (ReplyOk::HostMetrics(h.host_metrics()?), Vec::new()),

        ControlRequest::ReadDir { dir, root } => {
            let root = root.map(|r| p(&r));
            let entries = h.read_dir(&p(&dir), root.as_deref())?;
            (ReplyOk::Entries(entries), Vec::new())
        }
        ControlRequest::Stat { path } => (ReplyOk::Meta(h.stat(&p(&path))?), Vec::new()),
        ControlRequest::Exists { path } => (ReplyOk::Bool(h.exists(&p(&path))), Vec::new()),
        ControlRequest::Canonicalize { path } => {
            let canon = h.canonicalize(&p(&path))?;
            (
                ReplyOk::Path(canon.to_string_lossy().into_owned()),
                Vec::new(),
            )
        }
        ControlRequest::ReadLink { path } => {
            let target = h.read_link(&p(&path))?;
            let target = target.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "link target is not valid UTF-8")
            })?;
            (ReplyOk::Path(target.to_owned()), Vec::new())
        }
        ControlRequest::ReadFile { path, max_bytes } => {
            let path = p(&path);
            let bytes = h.read_file(&path, max_bytes)?;
            let meta = h.stat(&path)?;
            (ReplyOk::FileMeta { meta }, bytes)
        }
        ControlRequest::Search {
            roots,
            query,
            limit,
            max_dirs,
            show_hidden,
        } => {
            let roots: Vec<PathBuf> = roots.iter().map(|r| p(r)).collect();
            let mut hits = h.search(
                &roots,
                &query,
                clamp_usize(limit),
                clamp_usize(max_dirs),
                show_hidden,
            )?;
            drop_unsendable_hits(&mut hits);
            (ReplyOk::Hits(hits), Vec::new())
        }
        ControlRequest::SearchContent {
            roots,
            query,
            limits,
        } => {
            let roots: Vec<PathBuf> = roots.iter().map(|r| p(r)).collect();
            let mut found = h.search_content(&roots, &query, &limits)?;
            drop_unsendable_content_hits(&mut found.hits);
            (ReplyOk::ContentHits(found), Vec::new())
        }

        ControlRequest::WriteFile { path } => {
            (ReplyOk::Meta(h.write_file(&p(&path), &blob)?), Vec::new())
        }
        ControlRequest::CreateFileNew { path } => {
            h.create_file_new(&p(&path))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::CreateDir { path, recursive } => {
            h.create_dir(&p(&path), recursive)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::Rename { from, to } => {
            h.rename(&p(&from), &p(&to))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::Remove { path, recursive } => {
            h.remove(&p(&path), recursive)?;
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::RepoRoot { path } => {
            let root = h.repo_root(&p(&path))?;
            (
                ReplyOk::OptPath(root.map(|r| r.to_string_lossy().into_owned())),
                Vec::new(),
            )
        }
        ControlRequest::Git { cwd, args } => {
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            (ReplyOk::Output(h.git(&p(&cwd), &borrowed)?), Vec::new())
        }
        ControlRequest::GitStream { id, cwd, args } => {
            conn.start_git_stream(id, p(&cwd), args);
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::Shells => (ReplyOk::Shells(h.shells()?), Vec::new()),
        ControlRequest::AgentSessions => (ReplyOk::AgentSessions(h.agent_sessions()?), Vec::new()),
        ControlRequest::SessionOperation { request } => (
            ReplyOk::SessionOperation(Box::new(h.session_operation(&request))),
            Vec::new(),
        ),
        ControlRequest::OpenCodeSessions { request } => (
            ReplyOk::OpenCodeSessions(Box::new(h.opencode_sessions(&request))),
            Vec::new(),
        ),
        ControlRequest::OpenCodeStatistics { request } => (
            ReplyOk::OpenCodeStatistics(Box::new(h.opencode_statistics(&request))),
            Vec::new(),
        ),
        ControlRequest::HermesSessions { request } => (
            ReplyOk::HermesSessions(Box::new(h.hermes_sessions(&request))),
            Vec::new(),
        ),
        ControlRequest::HermesStatistics { request } => (
            ReplyOk::HermesStatistics(Box::new(h.hermes_statistics(&request))),
            Vec::new(),
        ),

        ControlRequest::WatchOpen { dirs } => {
            let id = conn.open_watch(req_id, &paths(&dirs))?;
            (ReplyOk::WatchId(id), Vec::new())
        }
        ControlRequest::WatchSet { id, dirs } => {
            conn.set_watch_dirs(id, &paths(&dirs))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::WatchClose { id } => {
            conn.close_watch(id);
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::WorkspaceAttach { id } => (
            ReplyOk::Attached {
                took_over_from: conn.identity.attach(conn.machine.as_deref(), &id, false)?,
            },
            Vec::new(),
        ),
        ControlRequest::WorkspaceDetach { id } => {
            conn.identity.detach(conn.machine.as_deref(), &id)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::GuiOpen { path, workspace } => (
            ReplyOk::Bool(conn.identity.open_gui(path, workspace)),
            Vec::new(),
        ),

        ControlRequest::MachineGet => (
            ReplyOk::MachineTree(Box::new(machine_with_live_panes(conn)?)),
            Vec::new(),
        ),
        ControlRequest::WorkspaceTree { workspace } => (
            ReplyOk::WorkspaceTree(Box::new(conn.machine()?.workspace(workspace)?)),
            Vec::new(),
        ),
        ControlRequest::WorkspaceSidecarGet { remote } => (
            ReplyOk::WorkspaceSidecar(conn.machine()?.sidecar_snapshot(&remote).map(Box::new)),
            Vec::new(),
        ),
        ControlRequest::WorkspaceSidecarApply {
            remote,
            updates,
            panes,
        } => {
            let closed = conn.machine()?.sidecar_apply(
                remote.clone(),
                updates,
                panes,
                conn.machine_origin,
            )?;
            if let Some(directory) = conn.panes.as_ref() {
                for pane in closed {
                    directory.close_pane(pane);
                }
            }
            (
                ReplyOk::WorkspaceSidecar(conn.machine()?.sidecar_snapshot(&remote).map(Box::new)),
                Vec::new(),
            )
        }
        ControlRequest::WorkspaceCreate { name, workspace } => (
            ReplyOk::WorkspaceTree(Box::new(conn.machine()?.workspace_create(
                workspace,
                name,
                conn.machine_origin,
            )?)),
            Vec::new(),
        ),
        ControlRequest::WorkspaceRename { workspace, name } => {
            conn.machine()?
                .workspace_rename(workspace, name, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::WorkspaceRemove { workspace } => {
            let store = conn.machine()?;
            let panes = conn
                .identity
                .remove_workspace(store, workspace, conn.machine_origin)?;
            (ReplyOk::Panes(panes), Vec::new())
        }
        ControlRequest::WorkspaceTouch { workspace } => {
            conn.machine()?
                .workspace_touch(workspace, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::WorkspaceSetActiveTab { workspace, tab } => {
            conn.machine()?
                .workspace_set_active_tab(workspace, tab, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabCreate {
            workspace,
            at,
            pane,
            tab,
        } => (
            ReplyOk::TabTree(Box::new(conn.machine()?.tab_create(
                workspace,
                at.map(clamp_usize),
                pane,
                tab,
                conn.machine_origin,
            )?)),
            Vec::new(),
        ),
        ControlRequest::TabClose { workspace, tab } => (
            ReplyOk::Panes(
                conn.machine()?
                    .tab_close(workspace, tab, conn.machine_origin)?,
            ),
            Vec::new(),
        ),
        ControlRequest::TabCloseRemembered {
            workspace,
            tab,
            panes,
        } => {
            let Some(directory) = conn.panes.as_ref() else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this peer serves no panes, so it cannot keep a closed tab to reopen",
                ));
            };
            let closed = conn.machine()?.tab_close_remembered(
                workspace,
                tab,
                &panes,
                crate::core::machine::unix_now(),
                conn.machine_origin,
            )?;
            // After the entry is in the tree, for the reason a sleeping tab's
            // panes are stopped after its mark: a pane stopped first reads, to
            // anyone looking in between, as one that died with nothing to
            // restore it for — and the sweeps take its screen.
            for pane in &closed.stopped {
                directory.hibernate_pane(*pane);
            }
            for pane in closed.strays.iter().chain(&closed.dropped) {
                directory.close_pane(*pane);
            }
            (ReplyOk::Panes(closed.stopped), Vec::new())
        }
        ControlRequest::TabReopen { workspace, tab } => {
            let (reopened, dropped) = conn.machine()?.tab_reopen(
                workspace,
                tab,
                crate::core::machine::unix_now(),
                conn.machine_origin,
            )?;
            if let Some(directory) = conn.panes.as_ref() {
                for pane in &dropped {
                    directory.close_pane(*pane);
                }
            }
            (ReplyOk::ReopenedTab(reopened.map(Box::new)), Vec::new())
        }
        ControlRequest::TabRename {
            workspace,
            tab,
            name,
        } => {
            conn.machine()?
                .tab_rename(workspace, tab, name, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabMove { workspace, tab, to } => {
            conn.machine()?
                .tab_move(workspace, tab, clamp_usize(to), conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabSetGroup {
            workspace,
            tab,
            group,
            last_auto,
        } => {
            conn.machine()?
                .tab_set_group(workspace, tab, group, last_auto, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::WorkspaceSetGroups { workspace, groups } => {
            conn.machine()?
                .workspace_set_groups(workspace, groups, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabSetHibernated {
            workspace,
            tab,
            hibernated,
        } => {
            let Some(directory) = conn.panes.as_ref() else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this peer serves no panes, so it cannot put a tab to sleep",
                ));
            };
            let panes = conn.machine()?.tab_set_hibernated(
                workspace,
                tab,
                hibernated,
                conn.machine_origin,
            )?;
            // After the mark, not before: a pane stopped under a tab the tree
            // does not yet call asleep is, to anyone reading the tree in
            // between, a pane that died — and a restore reading it that way
            // would spawn it straight back.
            if hibernated {
                for pane in &panes {
                    directory.hibernate_pane(*pane);
                }
            }
            (ReplyOk::Panes(panes), Vec::new())
        }
        ControlRequest::PaneSplit {
            workspace,
            pane,
            axis,
            ratio,
            new,
            first,
        } => {
            conn.machine()?.pane_split(
                workspace,
                pane,
                axis,
                ratio,
                new,
                first,
                conn.machine_origin,
            )?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneClose { workspace, pane } => (
            ReplyOk::Panes(
                conn.machine()?
                    .pane_close(workspace, pane, conn.machine_origin)?,
            ),
            Vec::new(),
        ),
        ControlRequest::PaneSetPresentation {
            workspace,
            pane,
            pip,
            fallback,
            restore,
        } => {
            conn.machine()?.pane_set_presentation(
                workspace,
                pane,
                pip,
                fallback,
                restore,
                conn.machine_origin,
            )?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PanePark {
            workspace,
            pane,
            restore,
        } => {
            conn.machine()?
                .pane_park(workspace, pane, restore, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneRestore {
            workspace,
            pane,
            fallback,
        } => {
            conn.machine()?
                .pane_restore(workspace, pane, fallback, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneSetPipGeometry {
            workspace,
            pane,
            geometry,
        } => {
            conn.machine()?.pane_set_pip_geometry(
                workspace,
                pane,
                geometry,
                conn.machine_origin,
            )?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneSetRatio {
            workspace,
            tab,
            path,
            ratio,
        } => {
            conn.machine()?
                .pane_set_ratio(workspace, tab, path, ratio, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneMove {
            workspace,
            pane,
            to,
            axis,
            first,
        } => {
            conn.machine()?
                .pane_move(workspace, pane, to, axis, first, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneReplace {
            workspace,
            old,
            new,
        } => {
            conn.machine()?
                .pane_replace(workspace, old, new, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::AgentStates => (
            ReplyOk::AgentStates(
                conn.panes
                    .as_ref()
                    .map(|p| p.agent_states())
                    .unwrap_or_default(),
            ),
            Vec::new(),
        ),
        ControlRequest::PaneProcs { pane_id } => (
            ReplyOk::PaneProcs(
                conn.panes
                    .as_ref()
                    .map(|p| p.pane_procs(pane_id))
                    .unwrap_or_default(),
            ),
            Vec::new(),
        ),
        ControlRequest::Routes => (
            ReplyOk::Routes(crate::daemon::ssh::SshManager::global().routes()),
            Vec::new(),
        ),
        ControlRequest::Status => (
            ReplyOk::Status(ServerStatus {
                pid: std::process::id(),
                uptime_secs: server_started().elapsed().as_secs(),
                panes: conn.panes.as_ref().map(|p| p.pane_count()).unwrap_or(0),
                control_version: CONTROL_VERSION,
                protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
                build: env!("CARGO_PKG_VERSION").to_string(),
                socket: control_endpoint_display(),
            }),
            Vec::new(),
        ),
    })
}


#[cfg(not(any(unix, windows)))]
fn control_endpoint_display() -> String {
    String::new()
}

fn paths(v: &[String]) -> Vec<PathBuf> {
    v.iter().map(PathBuf::from).collect()
}

fn clamp_usize(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

// Request/pool/stream state stays here until its own cutover. Lifecycle code
// uses this private Conn; peer identity and attachment transitions stay opaque.
struct Conn {
    host: SharedHost,
    sink: Arc<Sink>,
    inflight: Mutex<HashMap<u64, bool>>,
    watches: Mutex<HashMap<u64, WatchSub>>,
    deferred_watches: Mutex<HashMap<u64, (u64, smol::channel::Receiver<Vec<PathBuf>>)>>,
    next_watch: AtomicU64,
    git_streams: Arc<AtomicUsize>,
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


    fn is_cancelled(&self, req_id: u64) -> bool {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req_id)
            .copied()
            .unwrap_or(false)
    }

    fn forget(&self, req_id: u64) {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
    }

    fn cancel(&self, req_id: u64) {
        let mut f = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = f.get_mut(&req_id) {
            *flag = true;
        }
    }

    fn finish(&self, req_id: u64, reply: ControlReply, blob: Vec<u8>, wants_blob: bool) {
        let cancelled = self
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id)
            .unwrap_or(false);
        if cancelled {
            log::trace!("control request {req_id} was cancelled; dropping its reply");
            self.start_deferred_watch(req_id, false);
            return;
        }

        let msg = if wants_blob && matches!(reply, ControlReply::Ok(_)) {
            ControlServerMsg::ResponseBlob {
                req_id,
                reply,
                blob,
            }
        } else {
            ControlServerMsg::Response { req_id, reply }
        };
        let delivered = match msg.to_frame() {
            Ok((k, payload)) => match self.sink.send_frame(k, &payload) {
                Ok(()) => true,
                Err(e) => {
                    log::debug!("control reply {req_id} could not be written: {e}");
                    false
                }
            },
            Err(e) => {
                log::warn!("control reply {req_id} could not be encoded: {e}");
                let excuse = ControlServerMsg::Response {
                    req_id,
                    reply: ControlReply::Err(WireError::from_io(&e)),
                };
                if let Err(e) = self.sink.send(&excuse) {
                    log::debug!("control error reply {req_id} could not be written: {e}");
                }
                false
            }
        };

        self.start_deferred_watch(req_id, delivered);
    }

    fn open_watch(&self, req_id: u64, dirs: &[PathBuf]) -> io::Result<u64> {
        let sub = self.host.watch(dirs)?;
        let id = self.next_watch.fetch_add(1, Ordering::Relaxed);
        let rx = sub.events().clone();
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, sub);
        self.deferred_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(req_id, (id, rx));
        Ok(id)
    }

    fn start_deferred_watch(&self, req_id: u64, deliver: bool) {
        let parked = self
            .deferred_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
        let Some((id, rx)) = parked else {
            return;
        };
        if !deliver {
            self.watches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return;
        }
        spawn_watch_forwarder(id, rx, Arc::clone(&self.sink));
    }

    fn start_git_stream(&self, id: u64, cwd: PathBuf, args: Vec<String>) {
        let host = Arc::clone(&self.host);
        let sink = Arc::clone(&self.sink);
        let slot = StreamSlot(Arc::clone(&self.git_streams));
        if slot.0.fetch_add(1, Ordering::AcqRel) >= MAX_CONCURRENT_GIT_STREAMS {
            drop(slot);
            let _ = self
                .sink
                .send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code: None,
                    failed: true,
                }));
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("lyrel-control-git-stream".into())
            .spawn(move || {
                let _slot = slot;
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                let mut batch: Vec<u8> = Vec::with_capacity(GIT_STREAM_CHUNK);
                let mut stopped: Option<StreamStop> = None;
                let flush = |batch: &mut Vec<u8>, stopped: &mut Option<StreamStop>| {
                    if stopped.is_some() {
                        batch.clear();
                        return;
                    }
                    if batch.is_empty() {
                        return;
                    }
                    let bytes = std::mem::take(batch);
                    for piece in bytes.chunks(GIT_STREAM_CHUNK_MAX) {
                        let event = ControlServerMsg::Event(ControlEvent::GitChunk {
                            id,
                            bytes: piece.to_vec(),
                        });
                        let Ok((kind, payload)) = event.to_frame() else {
                            *stopped = Some(StreamStop::Unencodable);
                            return;
                        };
                        if sink.send_frame(kind, &payload).is_err() {
                            *stopped = Some(StreamStop::LinkGone);
                            return;
                        }
                    }
                };
                let result = host.git_lines(&cwd, &borrowed, &mut |line| {
                    if stopped.is_some() {
                        return;
                    }
                    batch.extend_from_slice(line.as_bytes());
                    batch.push(b'\n');
                    if batch.len() >= GIT_STREAM_CHUNK {
                        flush(&mut batch, &mut stopped);
                    }
                });
                flush(&mut batch, &mut stopped);
                if matches!(stopped, Some(StreamStop::LinkGone)) {
                    return;
                }
                let (code, failed) = match (stopped, result) {
                    (Some(StreamStop::Unencodable), _) => (None, true),
                    (_, Ok(code)) => (code, false),
                    (_, Err(_)) => (None, true),
                };
                let _ = sink.send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code,
                    failed,
                }));
            });
        if spawned.is_err() {
            let _ = self
                .sink
                .send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code: None,
                    failed: true,
                }));
        }
    }

    fn set_watch_dirs(&self, id: u64, dirs: &[PathBuf]) -> io::Result<()> {
        let watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
        let sub = watches.get(&id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no such watch subscription {id}"),
            )
        })?;
        sub.set_dirs(dirs)
    }

    fn close_watch(&self, id: u64) {
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
}

fn subscribe_machine(services: &Services, sink: &Arc<Sink>) -> Option<machine::Subscription> {
    let store = services.machine.as_ref()?;
    let (tx, rx) = smol::channel::bounded::<(String, machine::LayoutDelta)>(LAYOUT_EVENT_QUEUE);
    let lagged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_drop = Arc::clone(&lagged);
    let subscription = store.subscribe(Arc::new(
        move |workspace: &str, delta: &machine::LayoutDelta| {
            if tx.try_send((workspace.to_string(), delta.clone())).is_err() {
                saw_drop.store(true, Ordering::Release);
                log::warn!(
                    "dropping a layout delta for a peer {LAYOUT_EVENT_QUEUE} deltas behind; \
                     it will be told to resync"
                );
            }
        },
    ));
    spawn_layout_forwarder(rx, Arc::clone(sink), lagged);
    Some(subscription)
}

fn spawn_layout_forwarder(
    rx: smol::channel::Receiver<(String, machine::LayoutDelta)>,
    sink: Arc<Sink>,
    lagged: Arc<std::sync::atomic::AtomicBool>,
) {
    let spawned = std::thread::Builder::new()
        .name("lyrel-control-layout".into())
        .spawn(move || {
            while let Ok((workspace, delta)) = rx.recv_blocking() {
                if lagged.swap(false, Ordering::AcqRel) {
                    let mut superseded = 1;
                    while rx.try_recv().is_ok() {
                        superseded += 1;
                    }
                    log::info!(
                        "dropping {superseded} superseded layout delta(s) and asking the peer \
                         to resync"
                    );
                    if sink
                        .send(&ControlServerMsg::Event(ControlEvent::LayoutResync))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                let event = ControlEvent::Layout { workspace, delta };
                if sink.send(&ControlServerMsg::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the layout forwarder: {e}");
    }
}

fn spawn_watch_forwarder(id: u64, rx: smol::channel::Receiver<Vec<PathBuf>>, sink: Arc<Sink>) {
    let spawned = std::thread::Builder::new()
        .name("lyrel-control-watch".into())
        .spawn(move || {
            while let Ok(batch) = rx.recv_blocking() {
                let event = if batch.len() > WATCH_BURST_CAP {
                    ControlEvent::WatchOverflow { id }
                } else {
                    ControlEvent::Watch {
                        id,
                        paths: batch
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect(),
                    }
                };
                if sink.send(&ControlServerMsg::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the watch forwarder for subscription {id}: {e}");
    }
}

struct StreamSlot(Arc<AtomicUsize>);

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamStop {
    LinkGone,
    Unencodable,
}


type Job = Box<dyn FnOnce() + Send + 'static>;

struct Pool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    state: Mutex<PoolState>,
    wake: Condvar,
}

struct PoolState {
    jobs: VecDeque<Job>,
    workers: usize,
    idle: usize,
    closed: bool,
}

impl PoolState {
    fn wants_another_worker(&self) -> bool {
        self.jobs.len() > self.idle && self.workers < MAX_WORKERS
    }
}

impl Pool {
    fn new() -> Pool {
        Pool {
            inner: Arc::new(PoolInner {
                state: Mutex::new(PoolState {
                    jobs: VecDeque::new(),
                    workers: 0,
                    idle: 0,
                    closed: false,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    fn submit(&self, job: impl FnOnce() + Send + 'static) -> bool {
        let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.closed || st.jobs.len() >= MAX_QUEUED {
            return false;
        }
        st.jobs.push_back(Box::new(job));

        if st.wants_another_worker() {
            st.workers += 1;
            let inner = Arc::clone(&self.inner);
            match std::thread::Builder::new()
                .name("lyrel-control-worker".into())
                .spawn(move || worker(inner))
            {
                Ok(_) => return true,
                Err(e) => {
                    st.workers -= 1;
                    log::warn!("could not start a control worker: {e}");
                    if st.workers == 0 {
                        st.jobs.pop_back();
                        return false;
                    }
                }
            }
        }
        drop(st);
        self.inner.wake.notify_one();
        true
    }

    fn close(&self) {
        {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            st.closed = true;
            st.jobs.clear();
        }
        self.inner.wake.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

fn worker(inner: Arc<PoolInner>) {
    loop {
        let job = {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(job) = st.jobs.pop_front() {
                    break job;
                }
                if st.closed {
                    st.workers -= 1;
                    return;
                }
                st.idle += 1;
                let (guard, timeout) = inner
                    .wake
                    .wait_timeout(st, WORKER_LINGER)
                    .unwrap_or_else(|e| e.into_inner());
                st = guard;
                st.idle -= 1;
                if timeout.timed_out() && st.jobs.is_empty() {
                    st.workers -= 1;
                    return;
                }
            }
        };
        job();
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

#[cfg(test)]
mod aggregate_tests {
    use super::*;
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
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    #[test]
    fn the_pool_reuses_a_warm_worker() {
        let pool = Pool::new();

        let settled = || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                let st = pool.inner.state.lock().unwrap();
                if st.idle == st.workers {
                    return;
                }
                drop(st);
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("the pool never went idle");
        };

        for _ in 0..50 {
            let (tx, rx) = std::sync::mpsc::channel();
            assert!(pool.submit(move || {
                let _ = tx.send(());
            }));
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            settled();
        }
        let st = pool.inner.state.lock().unwrap();
        assert_eq!(
            st.workers, 1,
            "50 sequential jobs should not want 50 threads"
        );
    }

    #[test]
    fn a_worker_is_spawned_when_the_backlog_outruns_the_parked_workers() {
        let state = |jobs: usize, workers: usize, idle: usize| PoolState {
            jobs: (0..jobs).map(|_| Box::new(|| ()) as Job).collect(),
            workers,
            idle,
            closed: false,
        };

        assert!(!state(1, 1, 1).wants_another_worker());

        assert!(state(2, 1, 1).wants_another_worker());

        assert!(state(1, 1, 0).wants_another_worker());

        assert!(!state(64, MAX_WORKERS, 0).wants_another_worker());
    }

    #[test]
    fn the_pool_grows_for_concurrent_work() {
        let pool = Pool::new();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        for _ in 0..8 {
            let gate = Arc::clone(&gate);
            let done_tx = done_tx.clone();
            assert!(pool.submit(move || {
                let (lock, cv) = &*gate;
                let mut open = lock.lock().unwrap();
                let _ = done_tx.send(());
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }));
        }
        for _ in 0..8 {
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert_eq!(pool.inner.state.lock().unwrap().workers, 8);

        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        pool.close();
    }

    #[test]
    fn closing_the_pool_drops_queued_work() {
        let pool = Pool::new();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let ran = Arc::new(AtomicBool::new(false));

        let blocker = Arc::clone(&gate);
        assert!(pool.submit(move || {
            let (lock, cv) = &*blocker;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }));
        std::thread::sleep(Duration::from_millis(100));
        let ran2 = Arc::clone(&ran);
        pool.submit(move || ran2.store(true, Ordering::SeqCst));

        pool.close();
        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !ran.load(Ordering::SeqCst),
            "a job queued at close still ran"
        );
        assert!(!pool.submit(|| {}), "a closed pool must refuse work");
    }
}


#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use super::test_support::*;
    use std::io::Write;
    use crate::daemon::control::{ControlHello, MTime};
    use crate::host::local::LocalHost;
    use crate::host::remote::RemoteHost;
    use crate::host::{Entry, HostId, Meta, Output};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;


    struct SlowGit {
        inner: SharedHost,
        delay: Duration,
        running: Arc<AtomicBool>,
    }

    impl Host for SlowGit {
        fn id(&self) -> HostId {
            self.inner.id()
        }
        fn separator(&self) -> char {
            self.inner.separator()
        }
        fn is_absolute(&self, p: &Path) -> bool {
            self.inner.is_absolute(p)
        }
        fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
            self.inner.read_dir(dir, root)
        }
        fn stat(&self, p: &Path) -> io::Result<Meta> {
            self.inner.stat(p)
        }
        fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
            self.inner.read_file(p, max_bytes)
        }
        fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
            self.inner.canonicalize(p)
        }
        fn search(
            &self,
            roots: &[PathBuf],
            query: &str,
            limit: usize,
            max_dirs: usize,
            show_hidden: bool,
        ) -> io::Result<Vec<SearchHit>> {
            self.inner
                .search(roots, query, limit, max_dirs, show_hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &crate::host::ContentQuery,
            limits: &crate::host::ContentLimits,
        ) -> io::Result<crate::host::ContentResults> {
            self.inner.search_content(roots, query, limits)
        }
        fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
            self.inner.write_file(p, bytes)
        }
        fn create_file_new(&self, p: &Path) -> io::Result<()> {
            self.inner.create_file_new(p)
        }
        fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.inner.create_dir(p, recursive)
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.inner.rename(from, to)
        }
        fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.inner.remove(p, recursive)
        }
        fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
            self.inner.repo_root(p)
        }
        fn git(&self, _cwd: &Path, _args: &[&str]) -> io::Result<Output> {
            self.running.store(true, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            Ok(Output {
                status: Some(0),
                stdout: b"slow".to_vec(),
                stderr: Vec::new(),
            })
        }
        fn shells(&self) -> io::Result<crate::host::ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
            self.inner.watch(dirs)
        }
    }

    #[test]
    fn a_slow_request_does_not_block_a_fast_one() {
        let running = Arc::new(AtomicBool::new(false));
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(1500),
            running: Arc::clone(&running),
        }));

        let tmp = tempfile::TempDir::new().unwrap();
        let slow_host = Arc::clone(&p.host);
        let dir = tmp.path().to_path_buf();
        let slow = std::thread::spawn(move || slow_host.git(&dir, &["status"]));

        let deadline = Instant::now() + Duration::from_secs(5);
        while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(running.load(Ordering::SeqCst), "the slow request never ran");

        let started = Instant::now();
        p.host.stat(tmp.path()).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(600),
            "a stat behind an in-flight git took {elapsed:?} — the server is serializing"
        );

        let out = slow.join().unwrap().unwrap();
        assert_eq!(out.stdout, b"slow");
    }

    #[test]
    fn many_slow_requests_still_leave_the_server_answering() {
        let running = Arc::new(AtomicBool::new(false));
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(1200),
            running: Arc::clone(&running),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        let mut slow = Vec::new();
        for _ in 0..24 {
            let host = Arc::clone(&p.host);
            let dir = tmp.path().to_path_buf();
            slow.push(std::thread::spawn(move || {
                let _ = host.git(&dir, &["status"]);
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }

        let started = Instant::now();
        assert!(p.host.exists(tmp.path()));
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "24 in-flight gits stalled a trivial request"
        );
        for t in slow {
            let _ = t.join();
        }
    }

    #[test]
    fn a_short_deadline_gives_up_on_a_slow_git() {
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(300),
            running: Arc::new(AtomicBool::new(false)),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        let err = p
            .host
            .git_with_deadline(tmp.path(), &["status"], Duration::from_millis(100))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
    }

    #[test]
    fn a_long_deadline_outlasts_the_one_a_git_request_would_get() {
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(300),
            running: Arc::new(AtomicBool::new(false)),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        // The first call abandons its request mid-flight. The server has no
        // timer of its own — it runs the job to completion and answers into a
        // slot nobody is waiting on — so the point of doing it twice is that
        // the link is still usable afterwards. A `push` behind a cancelled
        // probe depends on exactly that.
        let _ = p
            .host
            .git_with_deadline(tmp.path(), &["status"], Duration::from_millis(100));
        let out = p
            .host
            .git_with_deadline(tmp.path(), &["status"], Duration::from_secs(5))
            .unwrap();
        assert_eq!(out.stdout, b"slow");
    }

    #[test]
    fn replies_come_back_out_of_order() {
        let running = Arc::new(AtomicBool::new(false));
        let p = pair_with(Arc::new(SlowGit {
            inner: LocalHost::new(),
            delay: Duration::from_millis(800),
            running: Arc::clone(&running),
        }));
        let tmp = tempfile::TempDir::new().unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let host = Arc::clone(&p.host);
        let dir = tmp.path().to_path_buf();
        let slow_tx = tx.clone();
        let slow = std::thread::spawn(move || {
            let _ = host.git(&dir, &["status"]);
            let _ = slow_tx.send("git");
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        p.host.stat(tmp.path()).unwrap();
        tx.send("stat").unwrap();

        assert_eq!(
            rx.recv().unwrap(),
            "stat",
            "the later request answered first"
        );
        assert_eq!(rx.recv().unwrap(), "git");
        slow.join().unwrap();
    }


    #[test]
    fn an_oversized_read_ships_no_bytes() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();
        let fat = tmp.path().join("fat.bin");
        std::fs::write(&fat, vec![b'x'; 4096]).unwrap();

        ControlClientMsg::Request {
            req_id: 1,
            req: ControlRequest::ReadFile {
                path: fat.to_string_lossy().into_owned(),
                max_bytes: 16,
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response { req_id, reply } => {
                assert_eq!(req_id, 1);
                match reply {
                    ControlReply::Err(e) => assert_eq!(e.kind, WireErrorKind::FileTooLarge),
                    other => panic!("{other:?}"),
                }
            }
            ControlServerMsg::ResponseBlob { blob, .. } => {
                panic!("the refusal carried {} bytes of the file", blob.len())
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_empty_read_still_uses_the_blob_kind() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();
        let empty = tmp.path().join("empty.txt");
        std::fs::write(&empty, b"").unwrap();

        ControlClientMsg::Request {
            req_id: 1,
            req: ControlRequest::ReadFile {
                path: empty.to_string_lossy().into_owned(),
                max_bytes: 1024,
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::ResponseBlob { reply, blob, .. } => {
                assert!(blob.is_empty());
                assert!(matches!(reply, ControlReply::Ok(ReplyOk::FileMeta { .. })));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn error_kinds_survive_the_round_trip() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let cases: Vec<(io::ErrorKind, io::Error)> = vec![
            (
                io::ErrorKind::NotFound,
                p.host.stat(&root.join("nope")).unwrap_err(),
            ),
            (io::ErrorKind::AlreadyExists, {
                let f = root.join("taken.txt");
                p.host.write_file(&f, b"x").unwrap();
                p.host.create_file_new(&f).unwrap_err()
            }),
            (io::ErrorKind::FileTooLarge, {
                let f = root.join("fat.txt");
                p.host.write_file(&f, &vec![b'x'; 512]).unwrap();
                p.host.read_file(&f, 16).unwrap_err()
            }),
            (io::ErrorKind::IsADirectory, {
                let d = root.join("adir");
                p.host.create_dir(&d, false).unwrap();
                p.host.read_file(&d, 1024).unwrap_err()
            }),
            (io::ErrorKind::DirectoryNotEmpty, {
                let d = root.join("full");
                p.host.create_dir(&d, false).unwrap();
                p.host.write_file(&d.join("f"), b"x").unwrap();
                p.host.remove(&d, false).unwrap_err()
            }),
        ];
        for (want, got) in cases {
            assert_eq!(got.kind(), want, "{got}");
            assert!(!got.to_string().is_empty());
        }

        for kind in [
            WireErrorKind::NotFound,
            WireErrorKind::PermissionDenied,
            WireErrorKind::AlreadyExists,
            WireErrorKind::InvalidInput,
            WireErrorKind::NotADirectory,
            WireErrorKind::IsADirectory,
            WireErrorKind::DirectoryNotEmpty,
            WireErrorKind::FileTooLarge,
            WireErrorKind::TimedOut,
            WireErrorKind::ConnectionReset,
            WireErrorKind::Other,
        ] {
            let io_err = io::Error::new(kind.to_io_kind(), "x");
            assert_eq!(
                WireError::from_io(&io_err).kind,
                kind,
                "{kind:?} is not its own round trip"
            );
        }
    }

    #[test]
    fn a_failing_git_is_ok_across_the_wire() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let Ok(out) = p.host.git(tmp.path(), &["rev-parse", "--show-toplevel"]) else {
            return;
        };
        if out.success() {
            return;
        }
        assert!(out.status.is_some());
        assert!(
            !out.stderr.is_empty(),
            "stderr crosses the wire, not dropped"
        );
    }


    #[test]
    fn a_cancelled_request_is_answered_with_silence() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();

        ControlClientMsg::Cancel { req_id: 1 }
            .encode(&mut client)
            .unwrap();
        ControlClientMsg::Request {
            req_id: 2,
            req: ControlRequest::Stat {
                path: tmp.path().to_string_lossy().into_owned(),
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response { req_id, reply } => {
                assert_eq!(req_id, 2);
                assert!(matches!(reply, ControlReply::Ok(ReplyOk::Meta(_))));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_machine_tree_is_declined_not_faked() {
        let p = pair();
        let err = p
            .host
            .client()
            .call(ControlRequest::MachineGet)
            .unwrap_err();
        assert!(err.to_string().contains("machine tree"), "{err}");
    }

    #[test]
    fn watch_batches_reach_the_client() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let sub = p.host.watch(&[tmp.path().to_path_buf()]).unwrap();
        while sub.events().try_recv().is_ok() {}

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut seen = false;
        while Instant::now() < deadline && !seen {
            std::fs::write(tmp.path().join("hello.txt"), b"x").unwrap();
            std::thread::sleep(Duration::from_millis(200));
            while let Ok(batch) = sub.events().try_recv() {
                if batch
                    .iter()
                    .any(|q| q.file_name().is_some_and(|n| n == "hello.txt"))
                {
                    seen = true;
                }
            }
        }
        assert!(seen, "no watch event crossed the connection");
    }

    #[test]
    fn a_watch_id_reaches_the_client_before_any_batch_for_it() {
        let (mut client, _ok) = raw();
        let tmp = tempfile::TempDir::new().unwrap();

        ControlClientMsg::Request {
            req_id: 1,
            req: ControlRequest::WatchOpen {
                dirs: vec![tmp.path().to_string_lossy().into_owned()],
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        let churn = tmp.path().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let churning = Arc::clone(&stop);
        let churner = std::thread::spawn(move || {
            let mut n = 0u32;
            while !churning.load(Ordering::SeqCst) {
                let _ = std::fs::write(churn.join(format!("f{n}")), b"x");
                n = n.wrapping_add(1);
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let first = ControlServerMsg::read(&mut client).unwrap();
        stop.store(true, Ordering::SeqCst);
        churner.join().unwrap();

        match first {
            ControlServerMsg::Response {
                req_id: 1,
                reply: ControlReply::Ok(ReplyOk::WatchId(_)),
            } => {}
            other => panic!("a batch overtook the WatchId reply: {other:?}"),
        }
    }

    #[test]
    fn closing_a_watch_releases_it_on_the_server() {
        let (server, client) = UnixStream::pair().unwrap();
        let host = LocalHost::new();
        std::thread::spawn(move || {
            let _ = serve(server, host);
        });
        let hello = ControlHello::host_rpc("t", "h");
        let remote = RemoteHost::over_unix(client, "test:watch", &hello).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let sub = remote.watch(&[tmp.path().to_path_buf()]).unwrap();
        let rx = sub.events().clone();
        drop(sub);

        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(tmp.path().join("after.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        while let Ok(batch) = rx.try_recv() {
            assert!(
                !batch
                    .iter()
                    .any(|q| q.file_name().is_some_and(|n| n == "after.txt")),
                "the server kept watching after the subscription closed"
            );
        }
    }

    #[test]
    fn a_gitignore_edit_reaches_a_remote_listing() {
        let p = pair();
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        p.host
            .write_file(&root.join(".gitignore"), b"*.log\n")
            .unwrap();
        p.host.write_file(&root.join("a.log"), b"").unwrap();

        let listed = p.host.read_dir(&root, Some(&root)).unwrap();
        assert!(
            listed.iter().any(|e| e.name == "a.log" && e.ignored),
            "the fixture should start out ignored"
        );

        let sub = p.host.watch(std::slice::from_ref(&root)).unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut cleared = false;
        while Instant::now() < deadline {
            p.host
                .write_file(&root.join(".gitignore"), b"# nothing\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(250));
            while sub.events().try_recv().is_ok() {}
            if p.host
                .read_dir(&root, Some(&root))
                .unwrap()
                .iter()
                .any(|e| e.name == "a.log" && !e.ignored)
            {
                cleared = true;
                break;
            }
        }
        assert!(
            cleared,
            "a `.gitignore` change on the server must reach the remote client's listing"
        );
    }

    #[test]
    fn a_write_answers_with_the_new_metadata() {
        let (mut client, _) = raw();
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("w.txt");

        ControlClientMsg::RequestBlob {
            req_id: 1,
            req: ControlRequest::WriteFile {
                path: f.to_string_lossy().into_owned(),
            },
            blob: b"hello".to_vec(),
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response {
                reply: ControlReply::Ok(ReplyOk::Meta(m)),
                ..
            } => {
                assert_eq!(m.len, 5);
                assert!(
                    m.mtime.is_some(),
                    "the editor keys its echo detection on this"
                );
                let on_disk = std::fs::metadata(&f).unwrap();
                assert_eq!(
                    MTime::from_system_time(on_disk.modified().unwrap()),
                    m.mtime.unwrap()
                );
            }
            other => panic!("{other:?}"),
        }
    }


    #[test]
    fn a_filename_that_is_not_utf8_costs_only_its_own_hit() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let hit = |name: &str, path: &Path| SearchHit {
            name: name.to_string(),
            path: path.to_path_buf(),
            is_dir: false,
            ignored: false,
        };
        let mut hits = vec![
            hit("plain-needle.rs", Path::new("/home/me/plain-needle.rs")),
            hit(
                "caf\u{fffd}-needle.rs",
                Path::new(OsStr::from_bytes(b"/home/me/caf\xe9-needle.rs")),
            ),
            hit("other-needle.rs", Path::new("/home/me/other-needle.rs")),
        ];

        drop_unsendable_hits(&mut hits);
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            ["plain-needle.rs", "other-needle.rs"],
            "the representable hits must survive their neighbour"
        );
    }


    #[test]
    fn a_stream_slot_comes_back_however_it_leaves() {
        let count = Arc::new(AtomicUsize::new(0));

        {
            let slot = StreamSlot(Arc::clone(&count));
            slot.0.fetch_add(1, Ordering::AcqRel);
            assert_eq!(count.load(Ordering::Acquire), 1);
        }
        assert_eq!(count.load(Ordering::Acquire), 0, "released on drop");

        let refused = StreamSlot(Arc::clone(&count));
        refused.0.fetch_add(1, Ordering::AcqRel);
        drop(refused);
        assert_eq!(count.load(Ordering::Acquire), 0);

        let panicking = Arc::clone(&count);
        let _ = std::thread::spawn(move || {
            let slot = StreamSlot(panicking);
            slot.0.fetch_add(1, Ordering::AcqRel);
            panic!("the read blew up");
        })
        .join();
        assert_eq!(
            count.load(Ordering::Acquire),
            0,
            "a panicking stream must not take its slot with it"
        );
    }

    #[test]
    fn git_stream_delivers_the_same_lines_as_the_buffered_read() {
        let (mut client, _hello) = raw();
        let here = env!("CARGO_MANIFEST_DIR");
        let args: Vec<String> = ["log", "--oneline", "-n", "30"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let buffered = match ask(
            &mut client,
            1,
            ControlRequest::Git {
                cwd: here.to_string(),
                args: args.clone(),
            },
        ) {
            ControlReply::Ok(ReplyOk::Output(o)) => o,
            other => panic!("expected output, got {other:?}"),
        };
        if buffered.status != Some(0) {
            return;
        }
        let expected: Vec<String> = String::from_utf8_lossy(&buffered.stdout)
            .lines()
            .map(str::to_string)
            .collect();

        ControlClientMsg::Request {
            req_id: 2,
            req: ControlRequest::GitStream {
                id: 77,
                cwd: here.to_string(),
                args,
            },
        }
        .encode(&mut client)
        .unwrap();
        client.flush().unwrap();

        let mut split = crate::core::git::LineSplitter::default();
        let mut got: Vec<String> = Vec::new();
        let mut accepted = false;
        let code = loop {
            match ControlServerMsg::read(&mut client).unwrap() {
                ControlServerMsg::Response { req_id: 2, reply } => {
                    assert!(
                        matches!(reply, ControlReply::Ok(ReplyOk::Unit)),
                        "{reply:?}"
                    );
                    accepted = true;
                }
                ControlServerMsg::Event(ControlEvent::GitChunk { id, bytes }) => {
                    assert_eq!(id, 77, "chunks carry the id the client chose");
                    split.push(&bytes, |l| got.push(l.to_string()));
                }
                ControlServerMsg::Event(ControlEvent::GitEnd { id, code, failed }) => {
                    assert_eq!(id, 77);
                    assert!(!failed, "git ran");
                    break code;
                }
                other => panic!("unexpected {other:?}"),
            }
        };
        split.finish(|l| got.push(l.to_string()));
        assert!(accepted, "the request was answered");
        assert_eq!(code, Some(0));
        assert_eq!(got, expected, "streamed lines match the buffered read");
        assert!(!got.is_empty(), "this repo has commits");
    }


    #[test]
    fn concurrent_connections_can_all_write_the_tree() {
        let (services, _dir) = workspace_services();
        let store = Arc::clone(services.machine.as_ref().unwrap());

        let writers: Vec<_> = (0..6)
            .map(|_| {
                let services = services.clone();
                std::thread::spawn(move || {
                    let (mut client, _) = raw_with(services);
                    for i in 0..10 {
                        let reply = ask(
                            &mut client,
                            i as u64 + 1,
                            ControlRequest::WorkspaceCreate {
                                name: Some(format!("w-{i}")),
                                workspace: None,
                            },
                        );
                        assert!(
                            matches!(reply, ControlReply::Ok(ReplyOk::WorkspaceTree(_))),
                            "{reply:?}"
                        );
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert_eq!(
            store.machine().workspaces.len(),
            60,
            "every operation landed exactly once"
        );
    }

    #[test]
    fn a_lagged_connection_hears_a_resync_instead_of_the_superseded_backlog() {
        let (server_end, mut client_end) = UnixStream::pair().unwrap();
        let sink = Arc::new(Sink::new(server_end));
        let (tx, rx) = smol::channel::bounded::<(String, machine::LayoutDelta)>(8);
        let lagged = Arc::new(AtomicBool::new(false));

        for _ in 0..4 {
            tx.send_blocking(("ws-1".to_string(), machine::LayoutDelta::WorkspaceDeleted))
                .unwrap();
        }
        lagged.store(true, Ordering::Release);
        spawn_layout_forwarder(rx, sink, Arc::clone(&lagged));

        assert_eq!(
            ControlServerMsg::read(&mut client_end).unwrap(),
            ControlServerMsg::Event(ControlEvent::LayoutResync)
        );
        assert!(
            !lagged.load(Ordering::Acquire),
            "the flag is consumed: one gap, one resync"
        );

        tx.send_blocking(("ws-2".to_string(), machine::LayoutDelta::WorkspaceDeleted))
            .unwrap();
        match ControlServerMsg::read(&mut client_end).unwrap() {
            ControlServerMsg::Event(ControlEvent::Layout { workspace, delta }) => {
                assert_eq!(workspace, "ws-2", "the stale backlog was delivered anyway");
                assert_eq!(delta, machine::LayoutDelta::WorkspaceDeleted);
            }
            other => panic!("expected the post-resync delta, got {other:?}"),
        }
    }

}
