//! Workspace membership, focus, and grouping through the single store transaction.

use super::{MachineStore, Persist, SubscriberId};
use crate::core::group_key::WorkspaceGroups;
use crate::core::machine::operations::{collect_orphan_panes, find_workspace, not_found, refuse};
use crate::core::machine::{LayoutDelta, MAX_WORKSPACES, TabId, Workspace, unix_now};
use crate::core::session::WorkspaceId;
use std::io;
use std::sync::Arc;

impl MachineStore {
    pub fn workspace_create(
        &self,
        id: Option<WorkspaceId>,
        name: Option<String>,
        origin: Option<SubscriberId>,
    ) -> io::Result<Workspace> {
        let created = self.mutate(origin, |m| {
            if m.workspaces.len() >= MAX_WORKSPACES {
                return Err(refuse(format!(
                    "this machine already holds {MAX_WORKSPACES} workspaces"
                )));
            }
            if let Some(id) = id
                && m.workspaces.iter().any(|w| w.id == id)
            {
                return Err(refuse(format!("workspace {id} already exists")));
            }
            let workspace = Workspace {
                id: id.unwrap_or_default(),
                name: name.clone(),
                ..Workspace::default()
            };
            m.workspaces.push(workspace.clone());
            Ok((
                workspace.clone(),
                vec![(
                    workspace.id,
                    LayoutDelta::WorkspaceCreated {
                        workspace: workspace.clone(),
                    },
                )],
            ))
        })?;
        Ok(created)
    }

    pub fn workspace_rename(
        &self,
        id: WorkspaceId,
        name: Option<String>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, id)?;
            ws.name = name.clone();
            Ok(((), vec![(id, LayoutDelta::WorkspaceRenamed { name })]))
        })
    }

    pub fn workspace_delete(
        &self,
        id: WorkspaceId,
        origin: Option<SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let index = m
                .workspaces
                .iter()
                .position(|w| w.id == id)
                .ok_or_else(|| not_found(format!("no workspace {id} on this machine")))?;
            m.workspaces.remove(index);
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            Ok((orphans, vec![(id, LayoutDelta::WorkspaceDeleted)]))
        })
    }

    pub fn workspace_touch(
        self: &Arc<Self>,
        id: WorkspaceId,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.ensure_flusher();
        self.mutate_with(origin, Persist::Soon, |m| {
            let ws = find_workspace(m, id)?;
            let now = unix_now();
            ws.last_active = now;
            Ok((
                (),
                vec![(id, LayoutDelta::WorkspaceTouched { last_active: now })],
            ))
        })
    }

    pub fn workspace_set_active_tab(
        &self,
        id: WorkspaceId,
        tab: TabId,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, id)?;
            if !ws.tabs.iter().any(|t| t.id == tab) {
                return Err(not_found(format!("workspace {id} has no tab {tab}")));
            }
            ws.active_tab = Some(tab);
            Ok(((), vec![(id, LayoutDelta::ActiveTabChanged { tab })]))
        })
    }

    /// Replaces the workspace's sidebar groups whole.
    ///
    /// Whole rather than one verb per edit: the set is small, edited by hand,
    /// and every edit to it — a pin, a rename, a drag that reorders — is
    /// "this is the list now". Last writer wins, which for two windows racing
    /// to fold the same header is the answer either of them would have given.
    ///
    /// A tab still pointing at a group this drops is handed back to auto
    /// grouping here, in the same mutation, so the tree never holds a tab
    /// filed under a group nobody can see. Each such tab is announced like any
    /// other regroup; the client that sent the new set is left out of those
    /// deltas as usual and has already cleared the tabs itself.
    pub fn workspace_set_groups(
        &self,
        workspace: WorkspaceId,
        groups: WorkspaceGroups,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let ws = find_workspace(m, workspace)?;
            let mut deltas = Vec::new();
            for t in &mut ws.tabs {
                if t.group.is_some_and(|g| !groups.contains(g)) {
                    t.group = None;
                    deltas.push((
                        workspace,
                        LayoutDelta::TabRegrouped {
                            tab: t.id,
                            group: None,
                            last_auto: t.last_auto.clone(),
                        },
                    ));
                }
            }
            ws.groups = groups.clone();
            deltas.push((workspace, LayoutDelta::GroupsChanged { groups }));
            Ok(((), deltas))
        })
    }
}
