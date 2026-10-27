use anyhow::{Result, bail};
use lyrel_core::core::machine::{PaneSeed, Workspace};
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::{Value, json};

use super::pane::hang_up_removed_panes;
use super::target::{fetch_machine, resolve_ws};
use super::{Outcome, report};
use crate::address::{self, Context, WorkspaceAddress};
use crate::backend::Backend;
use crate::cli::WsCmd;
use crate::{output, resolve};

pub(super) fn execute(cmd: WsCmd, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    match cmd {
        WsCmd::Ls => ws_ls(backend),
        WsCmd::Tree { ws } => ws_tree(ws.as_deref(), ctx, backend),
        WsCmd::New { name } => ws_new(name, backend),
        WsCmd::Rename { ws, name } => ws_rename(&ws, name, backend),
        WsCmd::Stop { .. } => bail!(
            "`lyrel ws stop` is not implemented yet — the control dialect has no \
             workspace-stop request; it arrives with the multi-subscriber slice"
        ),
        WsCmd::Rm { ws } => ws_rm(&ws, backend),
        WsCmd::Attach { ws } => ws_attach(address::parse_workspace(&ws), backend),
        WsCmd::Detach { ws } => ws_detach(&ws, backend),
    }
}

fn workspace_summary(ws: &Workspace) -> Value {
    json!({
        "id": ws.id.to_string(),
        "name": ws.name,
        "tabs": ws.tabs.len(),
        "panes": ws.tabs.iter().map(|t| t.root.pane_ids().len()).sum::<usize>(),
        "attached": ws.attachment.as_ref().map(|a| a.hostname.clone()),
    })
}

pub(super) fn ws_ls(backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let summaries: Vec<Value> = machine.user_workspaces().map(workspace_summary).collect();
    report(
        output::workspace_table(&machine),
        json!({ "workspaces": summaries }),
    )
}

fn ws_tree(explicit: Option<&str>, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let id = resolve_ws(explicit, ctx, &machine)?;
    match backend.control(ControlRequest::WorkspaceTree { workspace: id })? {
        ReplyOk::WorkspaceTree(ws) => report(
            output::workspace_tree(&ws, &machine),
            serde_json::to_value(&*ws)?,
        ),
        other => bail!("the server answered WorkspaceTree with {other:?}"),
    }
}

fn ws_new(name: Option<String>, backend: &mut dyn Backend) -> Result<Outcome> {
    match backend.control(ControlRequest::WorkspaceCreate {
        name,
        workspace: None,
    })? {
        ReplyOk::WorkspaceTree(ws) => report(
            ws.id.to_string(),
            json!({ "id": ws.id.to_string(), "name": ws.name }),
        ),
        other => bail!("the server answered WorkspaceCreate with {other:?}"),
    }
}

fn ws_rename(ws: &str, name: String, backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let id = resolve::workspace(&machine, &address::parse_workspace(ws))?.id;
    backend.control(ControlRequest::WorkspaceRename {
        workspace: id,
        name: Some(name.clone()),
    })?;
    report("", json!({ "id": id.to_string(), "name": name }))
}

fn ws_rm(ws: &str, backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let id = resolve::workspace(&machine, &address::parse_workspace(ws))?.id;
    let reply = backend.control(ControlRequest::WorkspaceRemove { workspace: id })?;
    hang_up_removed_panes("WorkspaceRemove", reply, backend)?;
    report("", json!({ "removed": id.to_string() }))
}

fn ws_attach(addr: WorkspaceAddress, backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let id = resolve::workspace(&machine, &addr)?.id;
    match backend.control(ControlRequest::WorkspaceAttach { id: id.to_string() })? {
        ReplyOk::Attached { took_over_from } => {
            let human = took_over_from
                .as_ref()
                .map(|host| format!("took over from {host}"))
                .unwrap_or_default();
            report(
                human,
                json!({ "attached": id.to_string(), "took_over_from": took_over_from }),
            )
        }
        other => bail!("the server answered WorkspaceAttach with {other:?}"),
    }
}

fn ws_detach(ws: &str, backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let id = resolve::workspace(&machine, &address::parse_workspace(ws))?.id;
    backend.control(ControlRequest::WorkspaceDetach { id: id.to_string() })?;
    report("", json!({ "detached": id.to_string() }))
}

pub(super) fn new_workspace(
    path: Option<String>,
    open: bool,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    let ws = match backend.control(ControlRequest::WorkspaceCreate {
        name: None,
        workspace: None,
    })? {
        ReplyOk::WorkspaceTree(ws) => *ws,
        other => bail!("the server answered WorkspaceCreate with {other:?}"),
    };
    let pane = backend.spawn_shell(ws.id, path.clone())?;
    backend.control(ControlRequest::TabCreate {
        workspace: ws.id,
        at: None,
        pane: PaneSeed {
            pane,
            cwd: path,
            ssh_spec: None,
            agent: None,
            shell: None,
        },
        tab: None,
    })?;
    // Only when asked: a workspace made from a script has no business
    // stealing the screen, and the switcher lists it either way.
    let opened = match open {
        false => false,
        true => match backend.control(ControlRequest::GuiOpen {
            path: None,
            workspace: Some(ws.id),
        }) {
            Ok(ReplyOk::Bool(opened)) => {
                // The workspace exists by the time we ask, so an unreachable
                // GUI is worth a word and not an exit code: failing here would
                // read as "nothing was made".
                if !opened {
                    eprintln!(
                        "lyrel: no GUI is running on this machine; \
                         the workspace was made all the same"
                    );
                }
                opened
            }
            Ok(other) => bail!("the server answered GuiOpen with {other:?}"),
            Err(error) => {
                eprintln!(
                    "lyrel: could not ask the GUI to open it ({error:#}); \
                     the workspace was made all the same"
                );
                false
            }
        },
    };
    report(
        ws.id.to_string(),
        json!({ "id": ws.id.to_string(), "pane": pane, "opened": opened }),
    )
}

#[cfg(test)]
mod tests;
