use anyhow::{Context as _, Result, bail};
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::json;

use super::target::{fetch_machine, resolve_ws};
use super::{Outcome, report};
use crate::address::Context;
use crate::backend::Backend;
use crate::cli::WorktreeCmd;

pub(super) fn execute(
    cmd: WorktreeCmd,
    machine: Option<&str>,
    ctx: &Context,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    if let Some(machine) = machine {
        bail!(
            "`lyrel worktree` works on this machine's repositories only — -m {machine} \
             would have run git here, not there"
        );
    }
    match cmd {
        WorktreeCmd::New {
            name,
            branch,
            from,
            agent,
            task,
            no_setup,
            ws,
            repo,
        } => worktree_new(
            WorktreeNew {
                name,
                branch,
                from,
                agent,
                task,
                no_setup,
                ws,
                repo,
            },
            ctx,
            backend,
        ),
        WorktreeCmd::Ls { repo } => worktree_ls(repo, backend),
        WorktreeCmd::Rm {
            target,
            force,
            close_panes,
            repo,
        } => worktree_rm(&target, force, close_panes, repo, backend),
    }
}

struct WorktreeNew {
    name: Option<String>,
    branch: Option<String>,
    from: Option<String>,
    agent: Option<String>,
    task: Option<String>,
    no_setup: bool,
    ws: Option<String>,
    repo: Option<std::path::PathBuf>,
}

fn repo_or_here(repo: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    match repo {
        Some(repo) => Ok(repo),
        None => std::env::current_dir().context("cannot read the current directory"),
    }
}

/// The same worktree the GUI's New Worktree dialog makes, with the same first
/// line typed into its tab. Setup runs unless `--no-setup`: whoever can run
/// this can already run the script by hand, so the GUI's approval step would
/// guard nothing here.
fn worktree_new(args: WorktreeNew, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    use lyrel_core::core::cli_agent::CLIAgent;
    use lyrel_core::core::worktree::{self, WorktreeRequest, setup};

    let host = lyrel_core::host::local::LocalHost::new();
    let repo = repo_or_here(args.repo)?;
    let agent = args
        .agent
        .as_deref()
        .map(|slug| {
            CLIAgent::from_slug(slug).ok_or_else(|| anyhow::anyhow!("no agent called \"{slug}\""))
        })
        .transpose()?;
    // Reject an undeliverable task before Git or the daemon can create anything.
    let overrides = lyrel_core::core::config::Config::load().agent_launch;
    let inventory = host.shells()?;
    let shell_program = inventory
        .shells
        .iter()
        .find(|shell| shell.label == inventory.default_name)
        .map_or(inventory.default_name.as_str(), |shell| {
            shell.program.as_str()
        });
    if shell_program.trim().is_empty() {
        bail!("the selected Host did not report its default shell");
    }
    let agent_line = agent
        .map(|a| {
            setup::agent_line(
                a,
                args.task.as_deref().unwrap_or(""),
                &overrides,
                shell_program,
            )
        })
        .transpose()
        .map_err(anyhow::Error::msg)?;
    // The workspace is settled before anything is created, so a bad --ws
    // leaves no worktree behind.
    let machine = fetch_machine(backend)?;
    let ws = resolve_ws(args.ws.as_deref(), ctx, &machine)?;
    let defaults = worktree::defaults(&*host, &repo).map_err(anyhow::Error::msg)?;
    let name = args.name.unwrap_or(defaults.name);
    let req = WorktreeRequest {
        branch: args.branch.unwrap_or_else(|| name.clone()),
        name,
        base: args.from.unwrap_or(defaults.base),
    };
    let wt = worktree::create(&*host, &repo, &req).map_err(anyhow::Error::msg)?;

    let setup = (!args.no_setup && cfg!(unix))
        .then(|| setup::find(&*host, &wt.path))
        .flatten()
        .zip(setup::env(&*host, &wt.main_root, &wt.path));
    let line = setup::launch_line(
        &wt.path,
        setup
            .as_ref()
            .map(|(s, env)| (s.script.as_path(), env.as_slice())),
        agent_line,
        shell_program,
    );

    let cwd = wt.path.to_string_lossy().into_owned();
    let pane = backend.spawn_shell(ws, Some(cwd.clone()))?;
    let tab = match backend.control(ControlRequest::TabCreate {
        workspace: ws,
        at: None,
        pane: PaneSeed {
            pane,
            cwd: Some(cwd.clone()),
            ssh_spec: None,
            agent: None,
            shell: None,
        },
        tab: None,
    })? {
        ReplyOk::TabTree(tab) => *tab,
        other => bail!("the server answered TabCreate with {other:?}"),
    };
    backend.control(ControlRequest::TabRename {
        workspace: ws,
        tab: tab.id,
        name: Some(wt.branch.clone()),
    })?;
    if let Some(line) = &line {
        backend.send_input(pane, format!("{line}\r").into_bytes())?;
    }

    let mut notes = vec![format!("branch {} from {}", wt.branch, req.base)];
    if !wt.carried.copied.is_empty() {
        notes.push(format!("copied {}", wt.carried.copied.join(", ")));
    }
    for (path, why) in &wt.carried.skipped {
        notes.push(format!("not copied {path}: {why}"));
    }
    notes.push(format!("pane %{pane}"));
    if setup.is_some() {
        notes.push("running .lyrel/setup".into());
    }
    report(
        format!("{cwd}\n  {}", notes.join(" · ")),
        json!({
            "path": cwd,
            "branch": wt.branch,
            "base": req.base,
            "tab": tab.id.to_string(),
            "pane": pane,
            "setup": setup.is_some(),
            "port": setup::port_base(&wt.path),
            "copied": wt.carried.copied,
            "skipped": wt.carried.skipped.iter().map(|(p, why)| json!({ "path": p, "reason": why })).collect::<Vec<_>>(),
            "command": line,
        }),
    )
}

fn worktree_ls(repo: Option<std::path::PathBuf>, backend: &mut dyn Backend) -> Result<Outcome> {
    let host = lyrel_core::host::local::LocalHost::new();
    let repo = repo_or_here(repo)?;
    let listed = lyrel_core::core::worktree::list(&*host, &repo).map_err(anyhow::Error::msg)?;
    // Each live pane belongs to the deepest checkout its cwd is in: the main
    // checkout contains `.lyrel/worktrees/*`, and a pane in one of those is
    // not also working in main.
    let mut owner: Vec<Vec<u64>> = vec![Vec::new(); listed.len()];
    for pane in backend.list_panes().unwrap_or_default() {
        let Some(cwd) = pane.cwd.as_deref().filter(|_| pane.alive) else {
            continue;
        };
        let deepest = listed
            .iter()
            .enumerate()
            .filter(|(_, wt)| cwd.starts_with(&wt.path))
            .max_by_key(|(_, wt)| wt.path.components().count());
        if let Some((i, _)) = deepest {
            owner[i].push(pane.pane_id);
        }
    }
    let mut human = Vec::new();
    let mut rows = Vec::new();
    for (wt, inside) in listed.iter().zip(owner) {
        let kind = match (wt.main, wt.managed) {
            (true, _) => "main",
            (false, true) => "lyrel",
            (false, false) => "",
        };
        let panes_text = inside
            .iter()
            .map(|p| format!("%{p}"))
            .collect::<Vec<_>>()
            .join(" ");
        human.push(format!(
            "{:<5} {:<28} {:<12} {}",
            kind,
            wt.branch.as_deref().unwrap_or("(detached)"),
            panes_text,
            wt.path.display()
        ));
        rows.push(json!({
            "path": wt.path,
            "branch": wt.branch,
            "main": wt.main,
            "managed": wt.managed,
            "panes": inside,
        }));
    }
    report(human.join("\n"), json!({ "worktrees": rows }))
}

fn worktree_rm(
    target: &str,
    force: bool,
    close_panes: bool,
    repo: Option<std::path::PathBuf>,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    use lyrel_core::core::worktree;

    let host = lyrel_core::host::local::LocalHost::new();
    let repo = repo_or_here(repo)?;
    let wt = worktree::find(&*host, &repo, target).map_err(anyhow::Error::msg)?;
    let occupying: Vec<u64> = backend
        .list_panes()?
        .into_iter()
        .filter(|p| p.alive && p.cwd.as_deref().is_some_and(|c| c.starts_with(&wt.path)))
        .map(|p| p.pane_id)
        .collect();
    if !occupying.is_empty() {
        let list = occupying
            .iter()
            .map(|p| format!("%{p}"))
            .collect::<Vec<_>>()
            .join(" ");
        if !close_panes {
            bail!(
                "{list} still working in {} — close them, or pass --close-panes",
                wt.path.display()
            );
        }
        // A dirty checkout is refused before anything is closed, so a
        // missing --force does not cost the panes.
        if wt.dirty && !force {
            bail!(
                "{} has uncommitted changes — pass --force to remove it anyway (they are snapshotted first)",
                wt.path.display()
            );
        }
        for pane in &occupying {
            backend.kill_pane(*pane)?;
        }
    }
    let removed = worktree::remove(&*host, &wt, force).map_err(|e| {
        anyhow::anyhow!(if wt.dirty && !force {
            format!("{e} — pass --force to remove it anyway (they are snapshotted first)")
        } else {
            e
        })
    })?;
    let mut notes = vec![format!("removed {}", wt.path.display())];
    if let Some(snapshot) = &removed.snapshot {
        notes.push(format!("last state in {snapshot}"));
    }
    if removed.branch_kept {
        notes.push(format!("kept branch {} (unmerged commits)", wt.branch));
    }
    report(
        notes.join(" · "),
        json!({
            "path": wt.path,
            "branch": wt.branch,
            "snapshot": removed.snapshot,
            "branch_kept": removed.branch_kept,
            "closed_panes": occupying,
        }),
    )
}

#[cfg(test)]
mod tests;
