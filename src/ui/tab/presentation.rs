use super::{Tab, TabSet};
use crate::core::session::{WorkspaceId, WorkspaceStore};
use crate::daemon::protocol::ShellSpec;
use crate::ui::pane::{Dir, Pane, PaneSlot};
use crate::ui::tree_sync::projection::{logical_node, slot_pane_id};
use gpui_kit::{App, Axis, EntityId, Window};
use lyrel_core::core::machine::{Axis as TreeAxis, PaneNode, Tab as TreeTab, TabPresentation};
use std::collections::HashMap;
fn pane_from_existing(node: &PaneNode, slots: &mut HashMap<u64, PaneSlot>) -> Option<Pane> {
    match node {
        PaneNode::Leaf { pane } => slots.remove(pane).map(Pane::Leaf),
        PaneNode::Split { axis, ratio, a, b } => Some(Pane::split_node(
            match axis {
                TreeAxis::Horizontal => gpui_kit::Axis::Horizontal,
                TreeAxis::Vertical => gpui_kit::Axis::Vertical,
            },
            *ratio,
            pane_from_existing(a, slots)?,
            pane_from_existing(b, slots)?,
        )),
    }
}

impl Tab {
    pub(crate) fn prepare_split(
        &self,
        spawn: Option<SpawnAs>,
        window: &Window,
        cx: &App,
    ) -> Option<SplitRequest> {
        let target = self
            .command_target(window, cx)
            .or_else(|| self.pane.first_leaf())?;
        let view = target.terminal()?.read(cx);
        let (spawn, opened) = match spawn {
            Some(spawn) => {
                let opened = match spawn {
                    SpawnAs::Ssh(_) => OpenedFrom::FreshSshHost,
                    SpawnAs::Shell(_) => OpenedFrom::LocalShell,
                };
                (spawn, opened)
            }
            None => match view.ssh_spec() {
                Some(spec) => (
                    SpawnAs::Ssh(crate::ui::ssh_connect::resolve_persisted_ssh_spec(spec, cx)),
                    OpenedFrom::SameSshHost,
                ),
                None => (SpawnAs::Shell(view.shell_spec()), OpenedFrom::LocalShell),
            },
        };
        let workspace = match &spawn {
            SpawnAs::Shell(_) => view.workspace().cloned(),
            SpawnAs::Ssh(_) => None,
        };
        Some(SplitRequest {
            tab: self.id(),
            target: target.entity_id(),
            spawn,
            cwd: inherited_start_dir(opened, view.cwd(), view.remote_context().is_some()),
            workspace,
        })
    }
    fn change_scoped_presentation(
        &mut self,
        pane: EntityId,
        fallback: Option<EntityId>,
        parked: Option<bool>,
        cx: &App,
    ) -> Option<bool> {
        use crate::ui::pane_sidecar::{install_presentation, mixed, scoped_pane, snapshot};
        if !mixed(self, cx) {
            return None;
        }
        let leaves = self.pane.leaves();
        let target = leaves
            .iter()
            .find(|slot| slot.entity_id() == pane)
            .and_then(|slot| scoped_pane(slot, cx))?;
        let fallback = fallback
            .and_then(|id| leaves.iter().find(|slot| slot.entity_id() == id))
            .and_then(|slot| scoped_pane(slot, cx));
        let mut logical = snapshot(self, cx)?;
        let changed = match parked {
            Some(true) => logical.park(target),
            Some(false) => logical.restore_parked(target, fallback),
            None => {
                let promote = (logical.presentation.pip().map(|pip| pip.pane()) != Some(target))
                    .then_some(target);
                logical.set_pip(promote, fallback)
            }
        };
        if !changed {
            return Some(false);
        }
        Some(install_presentation(self, logical, cx))
    }
    pub(crate) fn cycle_focus(&self, forward: bool, window: &Window, cx: &App) -> Option<PaneSlot> {
        let leaves = self.pane.leaves();
        if leaves.len() < 2 {
            return None;
        }
        let current = leaves
            .iter()
            .position(|slot| slot.contains_focused(window, cx))
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % leaves.len()
        } else {
            (current + leaves.len() - 1) % leaves.len()
        };
        Some(leaves[next].clone())
    }

    pub(crate) fn swap_pane(&mut self, forward: bool, window: &Window, cx: &App) -> bool {
        let len = self.pane.leaves().len();
        if len < 2 {
            return false;
        }
        let from = self.pane.focused_index(window, cx).unwrap_or(0);
        let to = if forward {
            (from + 1) % len
        } else {
            (from + len - 1) % len
        };
        self.pane.swap_leaf_indices(from, to)
    }

    pub(crate) fn resize_pane(&self, dir: Dir, step: f32, window: &Window, cx: &App) -> bool {
        self.pane.resize_focused_pane(dir, step, window, cx)
    }
    pub(crate) fn pane_command_state(&self, pane: EntityId, cx: &App) -> (bool, bool, bool, bool) {
        let terminal = self
            .pane
            .terminals()
            .into_iter()
            .find(|view| view.entity_id() == pane);
        let ready = terminal.is_some();
        let live = terminal
            .as_ref()
            .is_some_and(|view| !view.read(cx).ssh_disconnected());
        let multiple = self.pane.leaves().len() > 1;
        let pip = self
            .pip_slot(cx)
            .is_some_and(|slot| slot.entity_id() == pane);
        (ready, live, multiple, pip)
    }
    pub(crate) fn remap_slot(&mut self, old: EntityId, new: &PaneSlot, remote: bool, cx: &App) {
        let Some(slot) = self
            .pane
            .leaves()
            .into_iter()
            .find(|slot| slot.entity_id() == old)
        else {
            return;
        };
        if let Some(presentation) = &mut self.foreign_presentation {
            if let Some(old_key) = crate::ui::pane_sidecar::scoped_pane(&slot, cx) {
                let new_key = crate::ui::pane_sidecar::scoped_pane(new, cx);
                presentation.remap_panes(|key| if key == old_key { new_key } else { Some(key) });
            }
        }
        let old_id = match slot {
            PaneSlot::Ready(view) if remote && view.read(cx).pane_route().is_local() => None,
            PaneSlot::Ready(view) => Some(view.read(cx).pane_id),
            PaneSlot::Connecting(view) => view.read(cx).restore_pane(),
        };
        if let Some(old_id) = old_id {
            let new_id = slot_pane_id(new, cx);
            self.presentation
                .remap_panes(|id| if id == old_id { new_id } else { Some(id) });
        }
    }
    pub(crate) fn toggle_focused_pip(
        &mut self,
        workspace: WorkspaceId,
        window: &Window,
        cx: &App,
    ) -> Option<PaneSlot> {
        let target = self.command_target(window, cx)?;
        let restoring = self
            .pip_slot(cx)
            .is_some_and(|pip| pip.entity_id() == target.entity_id());
        let fallback = if restoring {
            self.pane
                .leaves()
                .into_iter()
                .find(|slot| slot.entity_id() != target.entity_id())
                .map(|slot| slot.entity_id())
        } else {
            Some(target.entity_id())
        };
        self.toggle_pip(target.entity_id(), fallback, workspace, cx)
            .then_some(target)
    }

    pub(crate) fn restore_pip(
        &mut self,
        workspace: WorkspaceId,
        window: &Window,
        cx: &App,
    ) -> Option<PaneSlot> {
        let pip = self.pip_slot(cx)?;
        let fallback = self
            .pane
            .focused_leaf(window, cx)
            .filter(|slot| slot.entity_id() != pip.entity_id())
            .or_else(|| {
                self.pane
                    .leaves()
                    .into_iter()
                    .find(|slot| slot.entity_id() != pip.entity_id())
            })
            .map(|slot| slot.entity_id());
        self.toggle_pip(pip.entity_id(), fallback, workspace, cx)
            .then_some(pip)
    }

    pub(crate) fn park(
        &mut self,
        target: EntityId,
        workspace: WorkspaceId,
        cx: &App,
    ) -> Option<Option<PaneSlot>> {
        let fallback = self
            .pane
            .leaves()
            .into_iter()
            .find(|slot| slot.entity_id() != target && !self.is_parked_slot(slot, cx));
        if !self.set_parked(
            target,
            fallback.as_ref().map(PaneSlot::entity_id),
            true,
            workspace,
            cx,
        ) {
            return None;
        }
        self.remember_fallback(fallback.as_ref());
        Some(fallback)
    }

    pub(crate) fn restore_parked(
        &mut self,
        target: EntityId,
        workspace: WorkspaceId,
        cx: &App,
    ) -> Option<PaneSlot> {
        let slot = self
            .pane
            .leaves()
            .into_iter()
            .find(|slot| slot.entity_id() == target)?;
        if !self.is_parked_slot(&slot, cx) {
            return None;
        }
        let fallback = self.focus_target(cx).map(|slot| slot.entity_id());
        self.set_parked(target, fallback, false, workspace, cx)
            .then_some(slot)
    }
    pub(crate) fn presentation(&self) -> &TabPresentation {
        &self.presentation
    }

    pub(crate) fn scoped_presentation(
        &self,
    ) -> Option<&TabPresentation<lyrel_core::core::pane_sidecar::ScopedPane>> {
        self.foreign_presentation.as_ref()
    }

    /// Hydration supplies machine-owned presentation; local focus stays intact.
    pub(crate) fn adopt_presentation(&mut self, presentation: TabPresentation) {
        self.presentation = presentation;
    }

    pub(crate) fn adopt_scoped_presentation(
        &mut self,
        presentation: TabPresentation<lyrel_core::core::pane_sidecar::ScopedPane>,
    ) {
        let mut primary =
            presentation.map_panes(|pane| (!pane.owner().is_client_local()).then_some(pane.pane()));
        if presentation
            .pip()
            .is_none_or(|pip| pip.pane().owner().is_client_local())
        {
            primary.set_pip_geometry(self.presentation.pip_geometry());
        }
        self.presentation = primary;
        self.foreign_presentation = Some(presentation);
    }

    pub(crate) fn commit_pip_geometry(
        &mut self,
        pane: EntityId,
        geometry: lyrel_core::core::machine::PipGeometry,
        cx: &App,
    ) -> bool {
        if !self
            .pip_slot(cx)
            .is_some_and(|slot| slot.entity_id() == pane)
        {
            return false;
        }
        if let Some(presentation) = &mut self.foreign_presentation {
            let changed = presentation.set_pip_geometry(Some(geometry));
            if presentation
                .pip()
                .is_some_and(|pip| !pip.pane().owner().is_client_local())
            {
                self.presentation.set_pip_geometry(Some(geometry));
            }
            changed
        } else {
            self.presentation.set_pip_geometry(Some(geometry))
        }
    }

    pub(crate) fn command_target(&self, window: &Window, cx: &App) -> Option<PaneSlot> {
        self.pane
            .focused_leaf(window, cx)
            .or_else(|| self.focus_target(cx))
    }

    pub(crate) fn split_pane(
        &mut self,
        target: EntityId,
        axis: Axis,
        before: bool,
        new: PaneSlot,
    ) -> bool {
        self.pane.split_leaf(target, axis, before, new)
    }

    pub(crate) fn detach_pane(&mut self, pane: EntityId) -> Option<PaneSlot> {
        let slot = self
            .pane
            .leaves()
            .into_iter()
            .find(|slot| slot.entity_id() == pane)?;
        self.pane.take_leaf(&slot)
    }
    pub(crate) fn drop_pane(
        &mut self,
        from: EntityId,
        zone: crate::ui::pane_drag::DropZone<EntityId>,
    ) -> Option<PaneSlot> {
        let leaves = self.pane.leaves();
        let here = |id| leaves.iter().find(|slot| slot.entity_id() == id).cloned();
        let moved = here(from)?;
        let zone = zone.map(here)?;
        crate::ui::pane_drag::apply(&mut self.pane, &moved, zone).then_some(moved)
    }

    pub(crate) fn directional_focus(
        &mut self,
        dir: Dir,
        window: &Window,
        cx: &App,
    ) -> Option<PaneSlot> {
        let from = self.pane.focused_leaf(window, cx)?;
        let back = self.focus_origin(from.entity_id(), dir);
        let target = self.pane.neighbor_in_dir(dir, back, window, cx)?;
        let live = self
            .pane
            .leaves()
            .iter()
            .map(PaneSlot::entity_id)
            .collect::<Vec<_>>();
        self.remember_focus_origin(from.entity_id(), target.entity_id(), dir, &live);
        Some(target)
    }

    /// The active zoom is a window rendering handle; the tab owns its transition.
    pub(crate) fn toggle_zoom(
        &mut self,
        zoom: &mut Option<gpui_kit::Entity<crate::terminal::view::TerminalView>>,
        window: &Window,
        cx: &App,
    ) -> Option<PaneSlot> {
        if let Some(leaf) = zoom.take() {
            self.remember_leaf(leaf.entity_id());
            return self.focus_target(cx);
        }
        if self.pane.leaves().len() < 2 {
            return None;
        }
        let leaf = self.command_target(window, cx)?.terminal()?.clone();
        *zoom = Some(leaf.clone());
        Some(PaneSlot::Ready(leaf))
    }
    /// Toggles presentation through the shared domain mutation. Every leaf, including
    /// client-local native SSH, stays in the logical tree and keeps its existing view.
    pub(crate) fn toggle_pip(
        &mut self,
        pane: gpui_kit::EntityId,
        fallback: Option<gpui_kit::EntityId>,
        workspace: WorkspaceId,
        cx: &App,
    ) -> bool {
        self.change_presentation(pane, fallback, None, workspace, cx)
    }

    pub(crate) fn set_parked(
        &mut self,
        pane: gpui_kit::EntityId,
        fallback: Option<gpui_kit::EntityId>,
        parked: bool,
        workspace: WorkspaceId,
        cx: &App,
    ) -> bool {
        self.change_presentation(pane, fallback, Some(parked), workspace, cx)
    }

    fn change_presentation(
        &mut self,
        pane: gpui_kit::EntityId,
        fallback: Option<gpui_kit::EntityId>,
        parked: Option<bool>,
        workspace: WorkspaceId,
        cx: &App,
    ) -> bool {
        let leaves = self.pane.leaves();
        let remote = WorkspaceStore::all(cx)
            .get(workspace)
            .is_some_and(|w| w.is_remote());
        if remote && let Some(changed) = self.change_scoped_presentation(pane, fallback, parked, cx)
        {
            return changed;
        }
        let Some(target_slot) = leaves.iter().find(|slot| slot.entity_id() == pane) else {
            return false;
        };
        if remote
            && matches!(target_slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
        {
            return false;
        }
        let Some(target) = slot_pane_id(target_slot, cx) else {
            return false;
        };
        let Some(root) = logical_node(&self.pane, remote, cx) else {
            return false;
        };
        // An unassigned connecting leaf must not disappear when a restore reshapes.
        let mut slots = HashMap::with_capacity(leaves.len());
        let mut client_local = Vec::new();
        for slot in &leaves {
            if remote
                && matches!(slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
            {
                client_local.push(slot.clone());
                continue;
            }
            let Some(id) = slot_pane_id(slot, cx) else {
                return false;
            };
            if slots.insert(id, slot.clone()).is_some() {
                return false;
            }
        }
        let fallback = fallback
        .and_then(|id| leaves.iter().find(|slot| slot.entity_id() == id))
        .filter(|slot| {
            !remote
                || !matches!(slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
        })
        .and_then(|slot| slot_pane_id(slot, cx));
        let mut logical = TreeTab {
            root,
            presentation: self.presentation.clone(),
            id: self.id(),
            name: None,
            group: None,
            last_auto: None,
            hibernated: false,
        };
        let changed = match parked {
            Some(true) => logical.park(target),
            Some(false) => logical.restore_parked(target, fallback),
            None => {
                let promote = (logical.presentation.pip().map(|pip| pip.pane()) != Some(target))
                    .then_some(target);
                logical.set_pip(promote, fallback)
            }
        };
        if !changed {
            return false;
        }
        if logical_node(&self.pane, remote, cx).as_ref() != Some(&logical.root) {
            let Some(pane) = pane_from_existing(&logical.root, &mut slots) else {
                return false;
            };
            self.pane = client_local.into_iter().fold(pane, |tree, slot| {
                Pane::split_node(gpui_kit::Axis::Horizontal, 0.5, tree, Pane::Leaf(slot))
            });
        }
        self.presentation = logical.presentation;
        true
    }
}

impl TabSet {
    pub(crate) fn select_pane(&mut self, pane: EntityId) -> Option<PaneSlot> {
        let (index, slot) = self.tabs.iter().enumerate().find_map(|(ix, tab)| {
            tab.pane
                .leaves()
                .into_iter()
                .find(|slot| slot.entity_id() == pane)
                .map(|slot| (ix, slot))
        })?;
        self.select(index);
        self.remember_leaf(pane);
        Some(slot)
    }

    pub(crate) fn merge_panes(
        &mut self,
        source: lyrel_core::core::machine::TabId,
        zone: crate::ui::pane_drag::DropZone<EntityId>,
    ) -> Option<(lyrel_core::core::machine::TabId, Option<PaneSlot>)> {
        let from = self.index_of(source)?;
        let host = self.tabs.get(self.active)?;
        if host.id() == source {
            return None;
        }
        let leaves = host.pane.leaves();
        let zone = zone.map(|id| leaves.iter().find(|slot| slot.entity_id() == id).cloned())?;
        let mut moved = self.remove(from);
        let sub = std::mem::replace(&mut moved.pane, Pane::Empty);
        let first = sub.first_leaf();
        let grafted = match self.active_mut() {
            Some(host) => crate::ui::pane_drag::graft(&mut host.pane, sub, zone),
            None => Err(sub),
        };
        if let Err(sub) = grafted {
            moved.pane = sub;
            self.insert(from, moved);
            return None;
        }
        let merged_id = moved.id();
        let host = self.active_mut().expect("active tab exists");
        match (host.code.as_deref_mut(), moved.code) {
            (None, code) => host.code = code,
            (Some(code), Some(theirs)) => code.adopt(&theirs.all_files()),
            (Some(_), None) => {}
        }
        if host.diff_overlay.is_none() {
            host.diff_overlay = moved.diff_overlay;
        }
        Some((merged_id, first))
    }
}
/// What to start in a pane that is about to be opened.
pub(crate) enum SpawnAs {
    /// A local shell; `None` is whatever the default one is.
    Shell(Option<ShellSpec>),
    Ssh(Box<crate::daemon::protocol::NativeSshSpec>),
}

/// How a pane being opened relates to the pane it was opened from — which is
/// what decides whether it may start in that pane's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenedFrom {
    /// A local shell.
    LocalShell,
    /// The source pane's own SSH host, dialled again (⌘T, ⌘D on an SSH pane).
    SameSshHost,
    /// A saved host or quick connect: a connection of its own, whatever pane
    /// happened to be in front when it was asked for.
    FreshSshHost,
}

/// The directory a pane opened from another one starts in.
///
/// `source_cwd` is the source pane's last reported directory, and on a remote
/// pane that is a path on the far host. It carries over only to a pane on the
/// same side of the link: a local shell takes a local pane's directory, and a
/// redial of the same host takes the far shell's. Anything else starts where a
/// fresh pane would — a local path handed to a remote `cd` names nothing there,
/// and a remote one handed to a local spawn names nothing here.
pub(crate) fn inherited_start_dir(
    opened: OpenedFrom,
    source_cwd: Option<std::path::PathBuf>,
    source_is_remote: bool,
) -> Option<std::path::PathBuf> {
    match opened {
        OpenedFrom::LocalShell => source_cwd.filter(|_| !source_is_remote),
        OpenedFrom::SameSshHost => source_cwd,
        OpenedFrom::FreshSshHost => None,
    }
}

/// A captured split destination and its Host-aware launch policy. Creation is
/// still performed by PendingPane; focus changes cannot change this destination.
pub(crate) struct SplitRequest {
    tab: lyrel_core::core::machine::TabId,
    target: EntityId,
    spawn: SpawnAs,
    cwd: Option<std::path::PathBuf>,
    workspace: Option<crate::terminal::PaneWorkspace>,
}

impl SplitRequest {
    pub(crate) fn into_parts(
        self,
    ) -> (
        lyrel_core::core::machine::TabId,
        EntityId,
        SpawnAs,
        Option<std::path::PathBuf>,
        Option<crate::terminal::PaneWorkspace>,
    ) {
        (self.tab, self.target, self.spawn, self.cwd, self.workspace)
    }
}
