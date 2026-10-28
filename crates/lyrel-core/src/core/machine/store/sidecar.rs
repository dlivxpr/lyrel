//! Client-local foreign projections still transact in the same authoritative store.

use std::io;

use super::{MachineStore, SubscriberId};
use crate::core::machine::operations::{not_found, references, refuse};
use crate::core::machine::{LayoutDelta, PaneSeed, Tab, Workspace};

impl MachineStore {
    pub fn sidecar_snapshot(
        &self,
        remote: &crate::core::session::RemoteRef,
    ) -> Option<crate::core::pane_sidecar::SidecarSnapshot> {
        let machine = self.locked();
        let workspace = machine
            .workspaces
            .iter()
            .find(|workspace| {
                workspace
                    .foreign_view
                    .as_ref()
                    .is_some_and(|view| view.remote() == remote)
            })?
            .clone();
        let ids: Vec<u64> = workspace
            .tabs
            .iter()
            .flat_map(|tab| tab.root.pane_ids())
            .collect();
        let panes = machine
            .panes
            .iter()
            .filter(|pane| ids.contains(&pane.id))
            .cloned()
            .collect();
        Some(crate::core::pane_sidecar::SidecarSnapshot::new(
            workspace, panes,
        ))
    }

    pub fn sidecar_apply(
        &self,
        remote: crate::core::session::RemoteRef,
        updates: Vec<crate::core::pane_sidecar::SidecarUpdate>,
        seeds: Vec<PaneSeed>,
        origin: Option<SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        use crate::core::pane_sidecar::ForeignWorkspace;
        self.mutate(origin, |machine| {
            let existing = machine.workspaces.iter().position(|workspace| {
                workspace
                    .foreign_view
                    .as_ref()
                    .is_some_and(|view| view.remote() == &remote)
            });
            let mut workspace = existing
                .map(|at| machine.workspaces[at].clone())
                .unwrap_or_else(|| Workspace {
                    foreign_view: Some(ForeignWorkspace::new(remote)),
                    ..Workspace::default()
                });
            let foreign = workspace
                .foreign_view
                .as_mut()
                .expect("sidecar owns a foreign view");
            let applied = foreign.apply(&updates)?;
            if !applied.changed {
                return Ok((Vec::new(), Vec::new()));
            }
            let local_tabs: Vec<Tab> = foreign
                .tabs()
                .iter()
                .filter_map(|tab| {
                    tab.map_panes(|pane| pane.owner().is_client_local().then_some(pane.pane()))
                })
                .collect();
            let owned: Vec<u64> = local_tabs
                .iter()
                .flat_map(|tab| tab.root.pane_ids())
                .collect();
            for pane in &owned {
                if machine.workspaces.iter().any(|other| {
                    other.id != workspace.id
                        && other.tabs.iter().any(|tab| tab.root.contains(*pane))
                }) {
                    return Err(refuse(format!(
                        "pane {pane} is already owned by another local workspace"
                    )));
                }
                if !machine.panes.iter().any(|record| record.id == *pane) {
                    let seed = seeds
                        .iter()
                        .find(|seed| seed.pane == *pane)
                        .cloned()
                        .ok_or_else(|| {
                            not_found(format!("no local pane record for sidecar pane {pane}"))
                        })?;
                    if !self.seed_is_live(*pane) {
                        return Err(not_found(format!("sidecar pane {pane} has exited")));
                    }
                    machine.panes.push(seed.into_record(true));
                }
            }
            workspace.tabs = local_tabs;
            workspace.active_tab = workspace.tabs.first().map(|tab| tab.id);
            if let Some(at) = existing {
                machine.workspaces[at] = workspace.clone();
            } else {
                machine.workspaces.push(workspace.clone());
            }
            let mut closed: Vec<u64> = applied
                .closed
                .into_iter()
                .filter(|pane| pane.owner().is_client_local())
                .map(|pane| pane.pane())
                .filter(|pane| !references(machine, *pane))
                .collect();
            closed.sort_unstable();
            closed.dedup();
            machine.panes.retain(|pane| !closed.contains(&pane.id));
            Ok((
                closed,
                vec![(workspace.id, LayoutDelta::WorkspaceCreated { workspace })],
            ))
        })
    }
}
