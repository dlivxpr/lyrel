use anyhow::{Result, bail};
use lyrel_core::core::machine::{Axis, Machine, PaneSeed};
use lyrel_core::core::session::WorkspaceId;
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::{Value, json};

use super::target::fetch_machine;
use super::{Outcome, Report, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::{PaneCmd, SplitArgs};
use crate::{output, resolve};

pub(super) fn execute(cmd: PaneCmd, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    match cmd {
        PaneCmd::Split(args) => pane_split(args, ctx, backend),
        PaneCmd::Ls { ws, all } => pane_ls(ws.as_deref(), all, backend),
        PaneCmd::Close { targets, orphans } => pane_close(&targets, orphans, ctx, backend),
    }
}

pub(super) fn pane_split(args: SplitArgs, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let pane = address::pane_or_context(args.target.as_deref(), ctx)?;
    let machine = fetch_machine(backend)?;
    let workspace = resolve::workspace_of_pane(&machine, pane)?.id;
    let cwd = machine
        .panes
        .iter()
        .find(|p| p.id == pane)
        .and_then(|p| p.cwd.clone());
    let axis = if args.horizontal {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    let new = backend.spawn_shell(workspace, cwd.clone())?;
    backend.control(ControlRequest::PaneSplit {
        workspace,
        pane,
        axis,
        ratio: args.ratio,
        new: PaneSeed {
            pane: new,
            cwd,
            ssh_spec: None,
            agent: None,
            shell: None,
        },
        first: false,
    })?;
    report(format!("%{new}"), json!({ "pane": new }))
}

pub(super) fn hang_up_removed_panes(request: &str, reply: ReplyOk, backend: &mut dyn Backend) -> Result<()> {
    let panes = match reply {
        ReplyOk::Panes(panes) => panes,
        other => bail!("the server answered {request} with {other:?}"),
    };
    let mut failures = Vec::new();
    for pane in panes {
        if let Err(error) = backend.kill_pane(pane) {
            failures.push(format!("%{pane}: {error:#}"));
        }
    }
    if !failures.is_empty() {
        bail!(
            "failed to hang up {} pane(s) removed by {request}: {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(())
}

fn pane_ls(explicit: Option<&str>, all: bool, backend: &mut dyn Backend) -> Result<Outcome> {
    if all {
        return pane_ls_all(backend);
    }
    let machine = fetch_machine(backend)?;
    let only = match explicit {
        Some(s) => Some(resolve::workspace(&machine, &address::parse_workspace(s))?.id),
        None => None,
    };
    let mut panes = Vec::new();
    for ws in &machine.workspaces {
        if only.is_some_and(|id| id != ws.id) {
            continue;
        }
        for tab in &ws.tabs {
            for pane in tab.root.pane_ids() {
                let record = machine.panes.iter().find(|p| p.id == pane);
                panes.push(json!({
                    "pane": pane,
                    "workspace": ws.id.to_string(),
                    "tab": tab.id.to_string(),
                    "cwd": record.and_then(|r| r.cwd.clone()),
                    "live": record.map(|r| r.live),
                    "hibernated": tab.hibernated,
                }));
            }
        }
    }
    report(
        output::pane_table(&machine, only),
        json!({ "panes": panes }),
    )
}

/// The registry's own list, annotated with the workspace holding each pane.
/// Panes with no holder are the ones every tree-walking listing misses: an
/// interrupted `lyrel run` leaves its pane running with nothing referencing it,
/// and until it shows up here there is no way to find or stop it.
fn pane_ls_all(backend: &mut dyn Backend) -> Result<Outcome> {
    let machine = fetch_machine(backend)?;
    let running = backend.list_panes()?;
    let held: Vec<(u64, WorkspaceId)> = machine
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter().map(move |tab| (ws.id, tab)))
        .flat_map(|(id, tab)| tab.root.pane_ids().into_iter().map(move |pane| (pane, id)))
        .collect();
    let holder = |pane: u64| held.iter().find(|(p, _)| *p == pane).map(|(_, ws)| *ws);

    let panes: Vec<Value> = running
        .iter()
        .map(|info| {
            json!({
                "pane": info.pane_id,
                "workspace": holder(info.pane_id).map(|ws| ws.to_string()),
                "orphan": holder(info.pane_id).is_none(),
                "owner": info.owner,
                "title": info.title,
                "cwd": info.cwd,
                "live": info.alive,
            })
        })
        .collect();
    let orphans = running
        .iter()
        .filter(|info| holder(info.pane_id).is_none())
        .count();
    let mut human = output::registry_table(&running, &|pane| holder(pane).map(|ws| ws.to_string()));
    if orphans > 0 {
        human.push_str(&format!(
            "\n{orphans} pane(s) held by no workspace — `lyrel tab new --pane %<id>` puts one \
             back in a tab, `lyrel pane close %<id>` stops one, `lyrel pane close --orphans` \
             stops all of them\n"
        ));
    }
    report(human, json!({ "panes": panes, "orphans": orphans }))
}

fn pane_close(
    targets: &[String],
    orphans: bool,
    ctx: &Context,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    // One tree read for the whole batch: it resolves the orphan set and then
    // every pane's owning workspace.
    let machine = fetch_machine(backend)?;
    let panes = if orphans {
        let found = orphan_panes(&machine, backend)?;
        if found.is_empty() {
            return report("no orphan panes\n", json!({ "closed": [] }));
        }
        found
    } else if targets.is_empty() {
        vec![address::pane_or_context(None, ctx)?]
    } else {
        targets
            .iter()
            .map(|t| address::pane_or_context(Some(t), ctx))
            .collect::<Result<Vec<_>>>()?
    };

    // Every pane is attempted even if an earlier one fails — a reaper that
    // stops at the first error leaves the rest of the leak in place, which is
    // the state the caller was trying to fix.
    let mut closed = Vec::new();
    let mut failures = Vec::new();
    // The running-pane registry, read lazily on the first direct kill: the
    // direct path is fire-and-forget (the daemon never says whether it knew
    // the pane), so the registry is the only way `%99` can fail instead of
    // reporting `{"closed":[99]}` for a pane that never existed (#588).
    let mut running: Option<Vec<u64>> = None;
    for pane in panes {
        let outcome = match resolve::workspace_of_pane(&machine, pane) {
            Ok(ws) => {
                let workspace = ws.id;
                match backend.control(ControlRequest::PaneClose { workspace, pane }) {
                    Ok(reply) => hang_up_removed_panes("PaneClose", reply, backend),
                    Err(e) => Err(e),
                }
            }
            // No workspace holds it, so PaneClose has nothing to route through.
            // Hang it up directly instead of refusing — this is exactly the
            // orphan `pane ls --all` points the user at.
            Err(_) => {
                if running.is_none() {
                    running = Some(
                        backend
                            .list_panes()?
                            .iter()
                            .map(|info| info.pane_id)
                            .collect(),
                    );
                }
                if running.as_ref().is_some_and(|ids| ids.contains(&pane)) {
                    // A pane that exits between the listing and the kill is
                    // gone either way, which is what closing it wanted.
                    backend.kill_pane(pane)
                } else {
                    Err(anyhow::anyhow!("no such pane"))
                }
            }
        };
        match outcome {
            Ok(()) => closed.push(pane),
            Err(e) => failures.push(format!("%{pane}: {e:#}")),
        }
    }

    if !failures.is_empty() {
        // Structured even here, for the reason `wait` is: the caller was
        // cleaning up, and what they need next is which panes are still theirs
        // to deal with — an anyhow error would leave `--json` holding prose.
        // The complaint goes to stderr all the same, so `-q` still reports it
        // and the exit code is not the only thing that says so.
        eprintln!(
            "lyrel: closed {} pane(s); {} could not be closed — {}",
            closed.len(),
            failures.len(),
            failures.join("; ")
        );
        return Ok(Outcome::Exit(
            1,
            Report {
                human: String::new(),
                json: json!({ "closed": closed, "failed": failures }),
            },
        ));
    }
    let human = match closed.as_slice() {
        // The single-pane case is the overwhelming one and has always been
        // silent on success; only a batch is worth narrating.
        [_] => String::new(),
        many => format!(
            "closed {} panes: {}\n",
            many.len(),
            many.iter()
                .map(|p| format!("%{p}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    };
    report(human, json!({ "closed": closed }))
}

/// The panes the daemon is running that no workspace's tab tree references.
fn orphan_panes(machine: &Machine, backend: &mut dyn Backend) -> Result<Vec<u64>> {
    let held: Vec<u64> = machine
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .flat_map(|tab| tab.root.pane_ids())
        .collect();
    Ok(backend
        .list_panes()?
        .iter()
        .map(|info| info.pane_id)
        .filter(|pane| !held.contains(pane))
        .collect())
}

#[cfg(test)]
mod tests;
