//! Pure machine-tree validation and bookkeeping; no locks, disk, or publication.

use std::io;

use crate::core::session::WorkspaceId;

use super::{CLOSED_TAB_TTL, MAX_CLOSED_TABS, MAX_PANES, Machine, PaneSeed, Tab, TabId, Workspace};

pub(super) fn refuse(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

pub(super) fn not_found(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, msg.into())
}

pub(super) fn find_workspace(m: &mut Machine, id: WorkspaceId) -> io::Result<&mut Workspace> {
    m.workspaces
        .iter_mut()
        .find(|w| w.id == id)
        .ok_or_else(|| not_found(format!("no workspace {id} on this machine")))
}

pub(super) fn find_tab(
    m: &mut Machine,
    workspace: WorkspaceId,
    tab: TabId,
) -> io::Result<&mut Tab> {
    let ws = find_workspace(m, workspace)?;
    ws.tabs
        .iter_mut()
        .find(|t| t.id == tab)
        .ok_or_else(|| not_found(format!("workspace {workspace} has no tab {tab}")))
}

pub(super) fn heal_active_tab(ws: &mut Workspace, removed: usize) -> Option<TabId> {
    let named = ws
        .active_tab
        .is_some_and(|active| ws.tabs.iter().any(|t| t.id == active));
    if named || ws.tabs.is_empty() {
        ws.active_tab = ws.active_tab.filter(|_| named);
        return None;
    }
    let active = ws.tabs[removed.min(ws.tabs.len() - 1)].id;
    ws.active_tab = Some(active);
    Some(active)
}

/// A pane registers once: a second tab, a second split, a second seed for the
/// same id is refused. The exception is a record nothing names any more — left
/// by a reopened tab ([`super::MachineStore::tab_reopen`]) — which the pane it was
/// kept for claims back, taking over what the tree still knew about it.
pub(super) fn register_pane(m: &mut Machine, seed: PaneSeed, live: bool) -> io::Result<()> {
    if references(m, seed.pane) {
        return Err(refuse(format!(
            "pane {} is already part of this machine's tree",
            seed.pane
        )));
    }
    if let Some(record) = m.panes.iter_mut().find(|p| p.id == seed.pane) {
        let kept = std::mem::replace(record, seed.into_record(live));
        record.title = kept.title;
        record.osc_title = kept.osc_title;
        if record.cwd.is_none() {
            record.cwd = kept.cwd;
        }
        if record.agent.is_none() {
            record.agent = kept.agent;
        }
        if record.shell.is_none() {
            record.shell = kept.shell;
        }
        return Ok(());
    }
    if m.panes.len() >= MAX_PANES {
        return Err(refuse(format!(
            "this machine's tree already references {MAX_PANES} panes"
        )));
    }
    m.panes.push(seed.into_record(live));
    Ok(())
}

/// Whether a tab names `pane` — an open one, or one a workspace keeps to
/// reopen. A closed tab's panes are still the tree's: their records and stored
/// screens are what the reopen is made of.
pub(super) fn references(m: &Machine, pane: u64) -> bool {
    m.workspaces.iter().any(|w| {
        w.tabs.iter().any(|t| t.root.contains(pane))
            || w.closed.iter().any(|c| c.tab.root.contains(pane))
    })
}

pub(super) fn collect_orphan_panes(m: &Machine) -> Vec<u64> {
    m.panes
        .iter()
        .map(|p| p.id)
        .filter(|id| !references(m, *id))
        .collect()
}

/// Trim every workspace's recently-closed list to what it may keep at `now`:
/// nothing older than [`CLOSED_TAB_TTL`], and the newest [`MAX_CLOSED_TABS`]
/// of the rest. Answers the panes of the entries dropped.
///
/// An entry stamped in the future — a clock set back since it closed — counts
/// as just closed rather than as expired: losing it to a clock change would be
/// a surprise, keeping it a little longer is not.
pub(super) fn prune_closed(m: &mut Machine, now: u64) -> Vec<u64> {
    let ttl = CLOSED_TAB_TTL.as_secs();
    let mut dropped = Vec::new();
    for ws in &mut m.workspaces {
        ws.closed.retain(|c| {
            let keep = now.saturating_sub(c.closed_at) < ttl;
            if !keep {
                dropped.extend(c.tab.root.pane_ids());
            }
            keep
        });
        let excess = ws.closed.len().saturating_sub(MAX_CLOSED_TABS);
        for c in ws.closed.drain(..excess) {
            dropped.extend(c.tab.root.pane_ids());
        }
    }
    dropped
}

pub(super) fn clamp_ratio(ratio: f32) -> io::Result<f32> {
    if !ratio.is_finite() {
        return Err(refuse("a split ratio must be a finite number"));
    }
    Ok(ratio.clamp(0.05, 0.95))
}
