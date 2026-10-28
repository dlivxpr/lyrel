//! Tab identity, ordering, hibernation, and recently-closed transactions.

use std::io;

use crate::core::group_key::{AutoKey, GroupId};
use crate::core::session::WorkspaceId;

use super::{MachineStore, SubscriberId};
use crate::core::machine::operations::{
    collect_orphan_panes, find_tab, find_workspace, heal_active_tab, not_found, prune_closed,
    references, refuse, register_pane,
};
use crate::core::machine::{
    CLOSED_TAB_TTL, ClosedTab, LayoutDelta, MAX_CLOSED_TABS, PaneSeed, Remembered, ReopenedTab, Tab,
    TabCloseReason, TabId,
};

impl MachineStore {
    pub fn tab_create(
        &self,
        workspace: WorkspaceId,
        at: Option<usize>,
        pane: PaneSeed,
        id: Option<TabId>,
        origin: Option<SubscriberId>,
    ) -> io::Result<Tab> {
        let live = self.seed_is_live(pane.pane);
        self.mutate(origin, |m| {
            if let Some(id) = id
                && m.workspaces
                    .iter()
                    .any(|w| w.tabs.iter().any(|t| t.id == id))
            {
                return Err(refuse(format!("tab {id} already exists")));
            }
            register_pane(m, pane.clone(), live)?;
            let ws = find_workspace(m, workspace)?;
            let mut tab = Tab::leaf(pane.pane);
            if let Some(id) = id {
                tab.id = id;
            }
            let tab = tab;
            let at = at.unwrap_or(ws.tabs.len()).min(ws.tabs.len());
            ws.tabs.insert(at, tab.clone());
            ws.active_tab = Some(tab.id);
            let active = tab.id;
            Ok((
                tab.clone(),
                vec![
                    (workspace, LayoutDelta::TabCreated { at, tab }),
                    (workspace, LayoutDelta::ActiveTabChanged { tab: active }),
                ],
            ))
        })
    }

    pub fn tab_close(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        origin: Option<SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let index = ws
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .ok_or_else(|| not_found(format!("workspace {workspace} has no tab {tab}")))?;
            ws.tabs.remove(index);
            let mut deltas = vec![(
                workspace,
                LayoutDelta::TabClosed {
                    tab,
                    reason: TabCloseReason::Explicit,
                },
            )];
            if let Some(active) = heal_active_tab(ws, index) {
                deltas.push((workspace, LayoutDelta::ActiveTabChanged { tab: active }));
            }
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            Ok((orphans, deltas))
        })
    }

    /// [`Self::tab_close`] for a tab the user closed and may want back: the tab
    /// leaves the workspace exactly as a close takes it, and lands at the end
    /// of the workspace's [`closed`](Workspace::closed) list instead of being
    /// forgotten, with its panes' records kept for the reopen to read.
    ///
    /// `held` is every pane the closing client had in the tab. Ones the tree
    /// never recorded there come back as strays, for the caller to end
    /// outright. The list is trimmed to [`MAX_CLOSED_TABS`] and
    /// [`CLOSED_TAB_TTL`] in the same mutation, measured from `now` (seconds
    /// since the epoch) so the rule can be tested without a clock.
    ///
    /// Other windows hear the same explicit `TabClosed` a plain close raises.
    pub fn tab_close_remembered(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        held: &[u64],
        now: u64,
        origin: Option<SubscriberId>,
    ) -> io::Result<Remembered> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let index = ws
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .ok_or_else(|| not_found(format!("workspace {workspace} has no tab {tab}")))?;
            let mut closed = ws.tabs.remove(index);
            // A tab closed asleep comes back awake: reopening is itself the
            // wake, through the same restore.
            closed.hibernated = false;
            let stopped = closed.root.pane_ids();
            ws.closed.push(ClosedTab {
                tab: closed,
                closed_at: now,
            });
            let mut deltas = vec![(
                workspace,
                LayoutDelta::TabClosed {
                    tab,
                    reason: TabCloseReason::Explicit,
                },
            )];
            if let Some(active) = heal_active_tab(ws, index) {
                deltas.push((workspace, LayoutDelta::ActiveTabChanged { tab: active }));
            }
            // The caller stops these panes as soon as this returns. The records
            // stay for the reopen, but must not go on saying the panes run:
            // `lyrel wait` reads `live` to tell a shell from an exited pane, and
            // would otherwise wait on a closed one forever.
            for record in m.panes.iter_mut().filter(|p| stopped.contains(&p.id)) {
                record.live = false;
            }
            let outgrown = prune_closed(m, now);
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            let strays = held
                .iter()
                .copied()
                .filter(|p| !stopped.contains(p) && !references(m, *p))
                .collect();
            let dropped = outgrown
                .into_iter()
                .filter(|p| orphans.contains(p))
                .collect();
            Ok((
                Remembered {
                    stopped,
                    strays,
                    dropped,
                },
                deltas,
            ))
        })
    }

    /// Take a closed tab back off the workspace's list: the one named, or the
    /// most recently closed. `None` when there is nothing (left) to reopen.
    ///
    /// The tab does not go back into the workspace here. The client that asked
    /// rebuilds it — every pane a fresh shell opening on its predecessor's
    /// screen — and pushes it as the new tab it then is, under the same id.
    ///
    /// The records stay in the pane list until that push lands, and past it:
    /// a record is what keeps the sweeps off a pane's stored screen, and the
    /// screen is read when the successor spawns, which for a remote pane can
    /// be a round trip after this answers. The next close collects them.
    ///
    /// Also trims the list, so an entry that has aged out is never handed back.
    /// The second half of the answer is the panes that trim dropped.
    pub fn tab_reopen(
        &self,
        workspace: WorkspaceId,
        tab: Option<TabId>,
        now: u64,
        origin: Option<SubscriberId>,
    ) -> io::Result<(Option<ReopenedTab>, Vec<u64>)> {
        self.mutate(origin, |m| {
            find_workspace(m, workspace)?;
            let outgrown = prune_closed(m, now);
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            let dropped: Vec<u64> = outgrown
                .into_iter()
                .filter(|p| orphans.contains(p))
                .collect();
            let ws = find_workspace(m, workspace)?;
            let at = match tab {
                Some(id) => ws.closed.iter().rposition(|c| c.tab.id == id),
                None => ws.closed.len().checked_sub(1),
            };
            let Some(at) = at else {
                return Ok(((None, dropped), Vec::new()));
            };
            let entry = ws.closed.remove(at);
            let ids = entry.tab.root.pane_ids();
            let panes = m
                .panes
                .iter()
                .filter(|p| ids.contains(&p.id))
                .cloned()
                .collect();
            let reopened = ReopenedTab {
                tab: entry.tab,
                closed_at: entry.closed_at,
                panes,
            };
            Ok(((Some(reopened), dropped), Vec::new()))
        })
    }

    /// Drop the closed tabs that have aged past [`CLOSED_TAB_TTL`], with their
    /// records. Answers the panes dropped, whose stored screens the caller
    /// forgets. Run by the daemon on a timer, so an entry expires on time
    /// whether or not anyone closes or reopens a tab.
    pub fn expire_closed_tabs(&self, now: u64) -> Vec<u64> {
        let expired = self.mutate(None, |m| {
            let outgrown = prune_closed(m, now);
            if outgrown.is_empty() {
                return Ok((Vec::new(), Vec::new()));
            }
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            let dropped = outgrown
                .into_iter()
                .filter(|p| orphans.contains(p))
                .collect();
            Ok((dropped, Vec::new()))
        });
        expired.unwrap_or_else(|e| {
            log::warn!("could not expire closed tabs: {e}");
            Vec::new()
        })
    }

    pub fn tab_rename(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        name: Option<String>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let t = find_tab(m, workspace, tab)?;
            t.name = name.clone();
            Ok(((), vec![(workspace, LayoutDelta::TabRenamed { tab, name })]))
        })
    }

    pub fn tab_move(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        to: usize,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let from = ws
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .ok_or_else(|| not_found(format!("workspace {workspace} has no tab {tab}")))?;
            let moved = ws.tabs.remove(from);
            let to = to.min(ws.tabs.len());
            ws.tabs.insert(to, moved);
            Ok(((), vec![(workspace, LayoutDelta::TabMoved { tab, to })]))
        })
    }

    /// Put a tab to sleep, or mark it awake again. Answers the panes the tab
    /// holds, which for a tab going to sleep are the ones whoever asked is now
    /// expected to stop — see [`ControlRequest::TabSetHibernated`].
    ///
    /// Only the mark changes here. The panes stay in the tab and in the pane
    /// list, so the facts a wake needs survive the processes: waking hands the
    /// same ids to the ordinary restore, which spawns successors and swaps
    /// them in with `pane_replace`. Setting the mark a tab already has is not
    /// a change and raises no delta.
    ///
    /// The delta is `TabRestructured` carrying the whole tab rather than a
    /// variant of its own, so a client that predates the mark still decodes
    /// it — it reads the same tab back, minus a field it does not know.
    ///
    /// [`ControlRequest::TabSetHibernated`]: crate::daemon::control::ControlRequest::TabSetHibernated
    pub fn tab_set_hibernated(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        hibernated: bool,
        origin: Option<SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let t = find_tab(m, workspace, tab)?;
            let panes = t.root.pane_ids();
            if t.hibernated == hibernated {
                return Ok((panes, Vec::new()));
            }
            t.hibernated = hibernated;
            let delta = LayoutDelta::TabRestructured {
                tab: t.clone(),
                pane: None,
            };
            Ok((panes, vec![(workspace, delta)]))
        })
    }

    pub fn tab_set_group(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        group: Option<GroupId>,
        last_auto: Option<AutoKey>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let t = find_tab(m, workspace, tab)?;
            t.group = group;
            t.last_auto = last_auto.clone();
            Ok((
                (),
                vec![(
                    workspace,
                    LayoutDelta::TabRegrouped {
                        tab,
                        group,
                        last_auto,
                    },
                )],
            ))
        })
    }
}
