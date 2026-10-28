//! Pane ownership, layout, and presentation transactions.

use std::io;

use crate::core::session::WorkspaceId;

use super::{MachineStore, SubscriberId};
use crate::core::machine::operations::{
    clamp_ratio, collect_orphan_panes, find_tab, find_workspace, heal_active_tab, not_found, refuse,
    register_pane,
};
use crate::core::machine::{
    Axis, LayoutDelta, PaneNode, PaneSeed, PipGeometry, PipPane, Side, TabCloseReason, TabId,
};

impl MachineStore {
    pub fn pane_split(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        axis: Axis,
        ratio: f32,
        new: PaneSeed,
        first: bool,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        let ratio = clamp_ratio(ratio)?;
        let live = self.seed_is_live(new.pane);
        self.mutate(origin, |m| {
            register_pane(m, new.clone(), live)?;
            let record = m
                .panes
                .iter()
                .find(|p| p.id == new.pane)
                .cloned()
                .expect("registered above");
            let ws = find_workspace(m, workspace)?;
            let tab = ws
                .tabs
                .iter_mut()
                .find(|t| t.root.contains(pane))
                .ok_or_else(|| {
                    not_found(format!("workspace {workspace} has no pane {pane} to split"))
                })?;
            tab.root.split_leaf(pane, new.pane, axis, ratio, first);
            let delta = LayoutDelta::TabRestructured {
                tab: tab.clone(),
                pane: Some(record),
            };
            Ok(((), vec![(workspace, delta)]))
        })
    }

    pub fn pane_close(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        origin: Option<SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let index = ws
                .tabs
                .iter()
                .position(|t| t.root.contains(pane))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {pane}")))?;
            if let Some(foreign) = ws.foreign_view.as_mut() {
                foreign.remove_pane(crate::core::pane_sidecar::ScopedPane::client_local(pane));
                ws.tabs = foreign
                    .tabs()
                    .iter()
                    .filter_map(|tab| {
                        tab.map_panes(|pane| pane.owner().is_client_local().then_some(pane.pane()))
                    })
                    .collect();
                if !ws.tabs.iter().any(|tab| Some(tab.id) == ws.active_tab) {
                    ws.active_tab = ws.tabs.first().map(|tab| tab.id);
                }
                let delta = LayoutDelta::WorkspaceCreated {
                    workspace: ws.clone(),
                };
                let orphans = collect_orphan_panes(m);
                m.panes.retain(|record| !orphans.contains(&record.id));
                return Ok((orphans, vec![(workspace, delta)]));
            }
            let mut deltas = Vec::new();
            ws.tabs[index].forget_presentation(pane);
            match ws.tabs[index].root.remove_leaf(pane) {
                None => {
                    let closed = ws.tabs.remove(index);
                    deltas.push((
                        workspace,
                        LayoutDelta::TabClosed {
                            tab: closed.id,
                            reason: TabCloseReason::Emptied,
                        },
                    ));
                    if let Some(active) = heal_active_tab(ws, index) {
                        deltas.push((workspace, LayoutDelta::ActiveTabChanged { tab: active }));
                    }
                }
                Some(true) => deltas.push((
                    workspace,
                    LayoutDelta::TabRestructured {
                        tab: ws.tabs[index].clone(),
                        pane: None,
                    },
                )),
                Some(false) => unreachable!("the tab was chosen because it contains the pane"),
            };
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            Ok((orphans, deltas))
        })
    }

    pub fn pane_set_presentation(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        pip: bool,
        fallback: Option<u64>,
        restore: Option<PipPane>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let tab = ws
                .tabs
                .iter_mut()
                .find(|tab| tab.root.contains(pane))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {pane}")))?;
            if pip
                && restore
                    .as_ref()
                    .is_some_and(|restore| restore.pane() != pane)
            {
                return Err(refuse("PiP restore metadata names a different pane"));
            }
            // A stale restore for another pane cannot dismiss the current PiP.
            if !pip
                && tab
                    .presentation
                    .pip()
                    .is_none_or(|current| current.pane() != pane)
            {
                return Ok(((), Vec::new()));
            }
            let changed = tab.set_pip_with_restore(pip.then_some(pane), fallback, restore);
            let deltas = if changed {
                vec![(
                    workspace,
                    LayoutDelta::TabRestructured {
                        tab: tab.clone(),
                        pane: None,
                    },
                )]
            } else {
                Vec::new()
            };
            Ok(((), deltas))
        })
    }

    pub fn pane_park(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        restore: Option<PipPane>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |machine| {
            let ws = find_workspace(machine, workspace)?;
            let tab = ws
                .tabs
                .iter_mut()
                .find(|tab| tab.root.contains(pane))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {pane}")))?;
            if restore.as_ref().is_some_and(|entry| entry.pane() != pane) {
                return Err(refuse("Park restore metadata names a different pane"));
            }
            let changed = tab.park_with_restore(pane, restore);
            let deltas = if changed {
                vec![(
                    workspace,
                    LayoutDelta::TabRestructured {
                        tab: tab.clone(),
                        pane: None,
                    },
                )]
            } else {
                Vec::new()
            };
            Ok(((), deltas))
        })
    }

    pub fn pane_restore(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        fallback: Option<u64>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |machine| {
            let ws = find_workspace(machine, workspace)?;
            let tab = ws
                .tabs
                .iter_mut()
                .find(|tab| tab.root.contains(pane))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {pane}")))?;
            let changed = tab.restore_parked(pane, fallback);
            let deltas = if changed {
                vec![(
                    workspace,
                    LayoutDelta::TabRestructured {
                        tab: tab.clone(),
                        pane: None,
                    },
                )]
            } else {
                Vec::new()
            };
            Ok(((), deltas))
        })
    }

    pub fn pane_set_pip_geometry(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        geometry: Option<PipGeometry>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        if geometry.is_some_and(|geometry| !geometry.is_valid()) {
            return Err(refuse("invalid PiP geometry"));
        }
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let tab = ws
                .tabs
                .iter_mut()
                .find(|tab| tab.presentation.pip().is_some_and(|pip| pip.pane() == pane))
                .ok_or_else(|| {
                    not_found(format!("workspace {workspace} has no PiP pane {pane}"))
                })?;
            let deltas = if tab.presentation.set_pip_geometry(geometry) {
                vec![(
                    workspace,
                    LayoutDelta::TabRestructured {
                        tab: tab.clone(),
                        pane: None,
                    },
                )]
            } else {
                Vec::new()
            };
            Ok(((), deltas))
        })
    }

    pub fn pane_set_ratio(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        path: Vec<Side>,
        ratio: f32,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        let ratio = clamp_ratio(ratio)?;
        self.mutate(origin, |m| {
            let t = find_tab(m, workspace, tab)?;
            match t.root.descend_mut(&path) {
                Some(PaneNode::Split { ratio: r, .. }) => *r = ratio,
                _ => {
                    return Err(refuse(format!(
                        "tab {tab} has no split at that path any more"
                    )));
                }
            }
            Ok((
                (),
                vec![(workspace, LayoutDelta::RatioChanged { tab, path, ratio })],
            ))
        })
    }

    pub fn pane_move(
        &self,
        workspace: WorkspaceId,
        pane: u64,
        to: u64,
        axis: Axis,
        first: bool,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        if pane == to {
            return Err(refuse("a pane cannot be moved next to itself"));
        }
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let from = ws
                .tabs
                .iter()
                .position(|t| t.root.contains(pane))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {pane}")))?;
            let dest = ws
                .tabs
                .iter()
                .position(|t| t.root.contains(to))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {to}")))?;

            let mut deltas: Vec<(WorkspaceId, LayoutDelta)> = Vec::new();
            if from != dest {
                ws.tabs[from].forget_presentation(pane);
            }
            match ws.tabs[from].root.remove_leaf(pane) {
                None => {
                    if from == dest {
                        return Err(refuse("a pane cannot be moved next to itself".to_string()));
                    }
                    let closed = ws.tabs.remove(from);
                    deltas.push((
                        workspace,
                        LayoutDelta::TabClosed {
                            tab: closed.id,
                            reason: TabCloseReason::Emptied,
                        },
                    ));
                    if let Some(active) = heal_active_tab(ws, from) {
                        deltas.push((workspace, LayoutDelta::ActiveTabChanged { tab: active }));
                    }
                }
                Some(true) if from != dest => {
                    deltas.push((
                        workspace,
                        LayoutDelta::TabRestructured {
                            tab: ws.tabs[from].clone(),
                            pane: None,
                        },
                    ));
                }
                Some(true) => {}
                Some(false) => unreachable!("the tab was chosen because it contains the pane"),
            }
            let dest_tab = ws
                .tabs
                .iter_mut()
                .find(|t| t.root.contains(to))
                .expect("the destination tab still exists; only the source tab can close");
            dest_tab.root.split_leaf(to, pane, axis, 0.5, first);
            deltas.push((
                workspace,
                LayoutDelta::TabRestructured {
                    tab: dest_tab.clone(),
                    pane: None,
                },
            ));
            Ok(((), deltas))
        })
    }

    pub fn pane_replace(
        &self,
        workspace: WorkspaceId,
        old: u64,
        new: PaneSeed,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        let live = self.seed_is_live(new.pane);
        self.mutate(origin, |m| {
            register_pane(m, new.clone(), live)?;
            let record = m
                .panes
                .iter()
                .find(|p| p.id == new.pane)
                .cloned()
                .expect("registered above");
            let ws = find_workspace(m, workspace)?;
            let tab = ws
                .tabs
                .iter_mut()
                .find(|t| t.root.contains(old))
                .ok_or_else(|| not_found(format!("workspace {workspace} has no pane {old}")))?;
            tab.replace_pane(old, new.pane);
            let delta = LayoutDelta::TabRestructured {
                tab: tab.clone(),
                pane: Some(record),
            };
            m.panes.retain(|p| p.id != old);
            Ok(((), vec![(workspace, delta)]))
        })
    }
}
