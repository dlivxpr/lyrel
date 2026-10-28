//! Explicit wire request dispatch through the negotiated connection.
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::machine;
use crate::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlReply, ControlRequest, ReplyOk, ServerStatus,
    WireError, WireErrorKind, server_started,
};
use crate::host::{Host, SearchHit};

use super::{Conn, control_endpoint_display};

pub(super) fn read_loop<R: Read>(r: &mut R, conn: &Arc<Conn>) -> io::Result<()> {
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
            ControlClientMsg::Cancel { req_id } => conn.requests.cancel(req_id),
        }
    }
}

fn submit(conn: &Arc<Conn>, req_id: u64, req: ControlRequest, blob: Vec<u8>) {
    conn.requests.register(req_id);

    let job_conn = Arc::clone(conn);
    let queued = conn
        .pool
        .submit(move || run_job(&job_conn, req_id, req, blob));
    if !queued {
        conn.requests.finish(
            &conn.sink,
            &conn.watches,
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
    if conn.requests.is_cancelled(req_id) {
        conn.requests.forget(req_id);
        return;
    }

    let wants_blob = req.returns_blob();
    let (reply, out_blob) = match run_request(conn, req_id, req, blob) {
        Ok((ok, bytes)) => (ControlReply::Ok(ok), bytes),
        Err(e) => (ControlReply::Err(WireError::from_io(&e)), Vec::new()),
    };
    conn.requests.finish(
        &conn.sink,
        &conn.watches,
        req_id,
        reply,
        out_blob,
        wants_blob,
    );
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
            conn.git_streams.start(&conn.host, &conn.sink, id, p(&cwd), args);
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
            let id = conn.watches.open(&conn.host, req_id, &paths(&dirs))?;
            (ReplyOk::WatchId(id), Vec::new())
        }
        ControlRequest::WatchSet { id, dirs } => {
            conn.watches.set_dirs(id, &paths(&dirs))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::WatchClose { id } => {
            conn.watches.close(id);
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

fn paths(v: &[String]) -> Vec<PathBuf> {
    v.iter().map(PathBuf::from).collect()
}

fn clamp_usize(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod aggregate_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod unix_tests;
