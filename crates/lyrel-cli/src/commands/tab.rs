use anyhow::{Result, bail};
use lyrel_core::core::machine::{Machine, PaneSeed};
use lyrel_core::core::session::WorkspaceId;
use lyrel_core::core::tab_view::tab_views_of;
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::{Value, json};

use super::pane::hang_up_removed_panes;
use super::target::{fetch_machine, resolve_ws};
use super::{Outcome, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::TabCmd;
use crate::{output, resolve};

pub(super) fn execute(cmd: TabCmd, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    match cmd {
        TabCmd::Ls { ws } => tab_ls(ws.as_deref(), ctx, backend),
        TabCmd::New { ws, cwd, pane } => tab_new(ws.as_deref(), cwd, pane.as_deref(), ctx, backend),
        TabCmd::Close { tab } => tab_close(&tab, backend),
        TabCmd::Rename { tab, name } => tab_rename(&tab, name, backend),
        TabCmd::Move { tab, index } => tab_move(&tab, index, backend),
    }
}

fn tab_ls(explicit: Option<&str>, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let id = resolve_ws(explicit, ctx, &machine)?;
    let ws = machine
        .user_workspaces()
        .find(|ws| ws.id == id)
        .expect("resolve_ws returned an id straight out of this machine");
    let views = tab_views_of(ws, &machine.panes);
    let rows: Vec<Vec<String>> = ws
        .tabs
        .iter()
        .zip(&views)
        .map(|(tab, view)| {
            vec![
                format!("@{}", resolve::ordinal_of(&machine, tab.id).unwrap_or(0)),
                output::tab_label(view),
                // Only a pinned group is the workspace's to report: an auto
                // group is worked out by the GUI from a probe of the tab's
                // cwd, which the tree does not hold.
                pinned_group_of(ws, tab)
                    .map(output::pinned_group_label)
                    .unwrap_or_else(|| "-".to_string()),
                tab.root.pane_ids().len().to_string(),
            ]
        })
        .collect();
    let tabs: Vec<Value> = ws
        .tabs
        .iter()
        .zip(&views)
        .map(|(tab, view)| {
            json!({
                "ordinal": resolve::ordinal_of(&machine, tab.id),
                "id": tab.id.to_string(),
                // `name` stays what someone actually named the tab — usually
                // nothing. `label` is what the table prints.
                "name": tab.name,
                "label": output::tab_label(view),
                "agent": view.agent.map(|a| a.display_name()),
                "group": pinned_group_of(ws, tab).map(|g| json!({
                    "id": g.id.to_string(),
                    "name": g.given_name(),
                    "folder": g.folder,
                })),
                "panes": tab.root.pane_ids(),
                "hibernated": tab.hibernated,
            })
        })
        .collect();
    report(
        output::table(&["TAB", "NAME", "GROUP", "PANES"], &rows),
        json!({ "workspace": id.to_string(), "tabs": tabs }),
    )
}

/// The pinned group `tab` is filed under, when it names one the workspace
/// still has. A tab naming a deleted group is an auto tab, the same way the
/// sidebar draws it.
fn pinned_group_of<'a>(
    ws: &'a lyrel_core::core::machine::Workspace,
    tab: &lyrel_core::core::machine::Tab,
) -> Option<&'a lyrel_core::core::group_key::PinnedGroup> {
    ws.groups.get(tab.group?)
}

fn tab_new(
    explicit: Option<&str>,
    cwd: Option<String>,
    adopt: Option<&str>,
    ctx: &Context,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let (id, pane, cwd) = match adopt {
        Some(spec) => adopt_pane(spec, explicit, cwd, ctx, &machine, backend)?,
        None => {
            let id = resolve_ws(explicit, ctx, &machine)?;
            let pane = backend.spawn_shell(id, cwd.clone())?;
            (id, pane, cwd)
        }
    };
    let tab = match backend.control(ControlRequest::TabCreate {
        workspace: id,
        at: None,
        pane: PaneSeed {
            pane,
            cwd,
            ssh_spec: None,
            agent: None,
            shell: None,
        },
        tab: None,
    })? {
        ReplyOk::TabTree(tab) => *tab,
        other => bail!("the server answered TabCreate with {other:?}"),
    };
    report(
        format!("%{pane}"),
        json!({ "tab": tab.id.to_string(), "pane": pane }),
    )
}

/// Works out which workspace a pane that is already running goes into, and what
/// to seed the tab around it with.
///
/// The seed is rebuilt from the **live pane registry**, not from the tree, and
/// that is the whole design of this verb. `tab_close` retains the panes it
/// orphaned out of `m.panes` at the same moment it drops the tab, so by the
/// time anyone wants a pane back the tree has forgotten its record — its cwd,
/// its title, the shell it was started with. The registry still has the pane,
/// because the pane is still running; it is the only place left that knows
/// anything about it.
///
/// What the registry does not carry is `ssh_spec`, `agent` or `shell`, so a
/// re-homed pane is seeded without them. That costs nothing while the shell
/// lives — the tab is a view onto a pty that is already there — and only shows
/// up if the pane later dies and something tries to restore it from the seed.
/// Reconstructing those from a running pty is a separate problem; a tab you can
/// see and close beats a shell nobody can reach.
fn adopt_pane(
    spec: &str,
    explicit: Option<&str>,
    cwd: Option<String>,
    ctx: &Context,
    machine: &Machine,
    backend: &mut dyn Backend,
) -> Result<(WorkspaceId, u64, Option<String>)> {
    let pane = address::parse_pane(spec)?;
    let running = backend.list_panes()?;
    let info = running
        .iter()
        .find(|info| info.pane_id == pane)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no pane %{pane} is running on this machine — \
                 `lyrel pane ls --all` lists every pane the server holds"
            )
        })?;
    if let Ok(holder) = resolve::workspace_of_pane(machine, pane) {
        bail!(
            "%{pane} is already in a tab of workspace {} — `lyrel pane split` adds \
             to that tab, and `lyrel tab new --pane` is for panes no tab holds",
            resolve::short_id(&holder.id)
        );
    }
    // A pane the tree still knows nothing about, addressed with no workspace,
    // goes back to the one it was spawned for: that is what `pane ls --all`
    // prints as its owner, and the shell being recovered from is by definition
    // not inside lyrel, so `$LYREL_WS` is not going to answer here.
    let id = match (explicit, ctx.ws.as_deref()) {
        (None, None) => owner_of(info, machine).ok_or_else(|| {
            anyhow::anyhow!(
                "%{pane} does not name a workspace that still exists — \
                 say which one to re-home it into: `lyrel tab new <workspace> --pane %{pane}`"
            )
        })?,
        _ => resolve_ws(explicit, ctx, machine)?,
    };
    // The recorded cwd is a courtesy for a later restore, not something the
    // running shell is moved to; an explicit `--cwd` overrides it.
    let cwd = cwd.or_else(|| {
        info.cwd
            .as_ref()
            .map(|dir| dir.display().to_string())
            .filter(|dir| !dir.is_empty())
    });
    Ok((id, pane, cwd))
}

/// The workspace a pane was spawned for, if it is still on this machine.
fn owner_of(
    info: &lyrel_core::daemon::protocol::PaneInfo,
    machine: &Machine,
) -> Option<WorkspaceId> {
    let owner = info.owner.as_deref()?;
    machine
        .user_workspaces()
        .find(|ws| ws.id.to_string() == owner)
        .map(|ws| ws.id)
}

fn tab_close(tab: &str, backend: &mut dyn Backend) -> Result<Outcome> {
    let addr = address::parse_tab(tab)?;
    let machine = fetch_machine(backend)?;
    let (workspace, tab) = resolve::tab(&machine, &addr)?;
    let reply = backend.control(ControlRequest::TabClose { workspace, tab })?;
    hang_up_removed_panes("TabClose", reply, backend)?;
    report("", json!({ "closed": tab.to_string() }))
}

fn tab_rename(tab: &str, name: String, backend: &mut dyn Backend) -> Result<Outcome> {
    let addr = address::parse_tab(tab)?;
    let machine = fetch_machine(backend)?;
    let (workspace, tab) = resolve::tab(&machine, &addr)?;
    backend.control(ControlRequest::TabRename {
        workspace,
        tab,
        name: Some(name.clone()),
    })?;
    report("", json!({ "tab": tab.to_string(), "name": name }))
}

fn tab_move(tab: &str, index: u64, backend: &mut dyn Backend) -> Result<Outcome> {
    let addr = address::parse_tab(tab)?;
    let machine = fetch_machine(backend)?;
    let (workspace, tab) = resolve::tab(&machine, &addr)?;
    backend.control(ControlRequest::TabMove {
        workspace,
        tab,
        to: index,
    })?;
    report("", json!({ "tab": tab.to_string(), "to": index }))
}

#[cfg(test)]
mod tests;
