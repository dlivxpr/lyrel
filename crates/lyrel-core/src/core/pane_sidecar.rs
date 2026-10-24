//! Machine-owned composition of panes whose sessions belong to different daemons.

use std::io;

use serde::{Deserialize, Serialize};

use super::machine::{Axis, PaneNode, Tab, TabId};
use super::session::RemoteRef;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidecarSnapshot {
    workspace: super::machine::Workspace,
    panes: Vec<super::machine::PaneRecord>,
}

impl SidecarSnapshot {
    pub fn new(
        workspace: super::machine::Workspace,
        panes: Vec<super::machine::PaneRecord>,
    ) -> Self {
        Self { workspace, panes }
    }
    pub fn into_parts(self) -> (super::machine::Workspace, Vec<super::machine::PaneRecord>) {
        (self.workspace, self.panes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneOwner {
    Workspace,
    ClientLocal,
}

impl PaneOwner {
    pub fn is_client_local(self) -> bool {
        self == Self::ClientLocal
    }
}

/// A real daemon identity, relative to a sidecar's local owner and remote route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopedPane {
    owner: PaneOwner,
    pane: u64,
}

impl ScopedPane {
    pub const fn workspace(pane: u64) -> Self {
        Self {
            owner: PaneOwner::Workspace,
            pane,
        }
    }
    pub const fn client_local(pane: u64) -> Self {
        Self {
            owner: PaneOwner::ClientLocal,
            pane,
        }
    }
    pub const fn owner(self) -> PaneOwner {
        self.owner
    }
    pub const fn pane(self) -> u64 {
        self.pane
    }
}

/// The local daemon owns this composition, not the sessions referenced remotely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignWorkspace {
    remote: RemoteRef,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    tabs: Vec<Tab<ScopedPane>>,
    #[serde(default)]
    applied: std::collections::VecDeque<uuid::Uuid>,
}

#[derive(Debug, Default)]
pub struct SidecarApplied {
    pub changed: bool,
    /// Explicitly closed leaves, not leaves removed by a transfer or reorder.
    pub closed: Vec<ScopedPane>,
}

impl ForeignWorkspace {
    pub fn new(remote: RemoteRef) -> Self {
        Self {
            remote,
            revision: 0,
            tabs: Vec::new(),
            applied: Default::default(),
        }
    }
    pub fn remote(&self) -> &RemoteRef {
        &self.remote
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn tabs(&self) -> &[Tab<ScopedPane>] {
        &self.tabs
    }

    pub(crate) fn remove_pane(&mut self, pane: ScopedPane) {
        self.tabs.retain_mut(|tab| {
            if !tab.root.contains(pane) {
                return true;
            }
            tab.forget_presentation(pane);
            tab.root.remove_leaf(pane).is_some()
        });
        self.revision = self.revision.saturating_add(1);
    }

    /// Replays changes against the latest tree. An old client's unchanged fields
    /// never replace newer changes another client made while its pull was in flight.
    pub fn apply(&mut self, updates: &[SidecarUpdate]) -> io::Result<SidecarApplied> {
        let before = self.tabs.clone();
        let mut accepted = Vec::new();
        let mut closed = Vec::new();
        for update in updates {
            if self.applied.contains(&update.id) || accepted.contains(&update.id) {
                continue;
            }
            accepted.push(update.id);
            match &update.change {
                SidecarChange::CloseTab { tab } => {
                    self.tabs.retain(|candidate| {
                        if candidate.id != *tab {
                            return true;
                        }
                        closed.extend(candidate.root.pane_ids());
                        false
                    });
                }
                SidecarChange::Reconcile {
                    base,
                    desired,
                    transfer,
                } => {
                    if base.as_ref().is_some_and(|base| base.id != desired.id) {
                        self.tabs = before;
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "sidecar edit changes tab identity",
                        ));
                    }
                    // A reconcile edits an existing tab; only a seed may create
                    // one. Reject before transfer can steal another tab's leaves.
                    if base.is_some() && !self.tabs.iter().any(|tab| tab.id == desired.id) {
                        continue;
                    }
                    if *transfer {
                        let mut arriving = desired.root.pane_ids();
                        // A stale base may still name leaves already moved by
                        // another client. Only newly added leaves may transfer.
                        if let Some(base) = base {
                            arriving.retain(|pane| !base.root.contains(*pane));
                        }
                        self.tabs.retain_mut(|tab| {
                            if tab.id == desired.id {
                                return true;
                            }
                            for pane in &arriving {
                                if !tab.root.contains(*pane) {
                                    continue;
                                }
                                if tab.root.remove_leaf(*pane).is_none() {
                                    return false;
                                }
                            }
                            tab.presentation
                                .remap_panes(|pane| tab.root.contains(pane).then_some(pane));
                            true
                        });
                    }
                    match self
                        .tabs
                        .iter_mut()
                        .find(|candidate| candidate.id == desired.id)
                    {
                        Some(current) => merge_tab(current, base.as_ref(), desired),
                        None => self.tabs.push(desired.clone()),
                    }
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        if self
            .tabs
            .iter()
            .flat_map(|tab| tab.root.pane_ids())
            .any(|pane| !seen.insert(pane))
        {
            self.tabs = before;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a routed pane belongs to more than one sidecar leaf",
            ));
        }
        if accepted.is_empty() {
            return Ok(SidecarApplied::default());
        }
        for id in accepted {
            self.applied.push_back(id);
            if self.applied.len() > 512 {
                self.applied.pop_front();
            }
        }
        self.revision = self.revision.saturating_add(1);
        Ok(SidecarApplied {
            changed: true,
            closed,
        })
    }
}

/// Carries a client's semantic edit, including its base for field-level replay.
/// This is not a compare-and-swap of the entire sidecar document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidecarUpdate {
    id: uuid::Uuid,
    change: SidecarChange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum SidecarChange {
    Reconcile {
        base: Option<Tab<ScopedPane>>,
        desired: Tab<ScopedPane>,
        transfer: bool,
    },
    CloseTab {
        tab: TabId,
    },
}

impl SidecarUpdate {
    pub fn new(base: Option<Tab<ScopedPane>>, desired: Tab<ScopedPane>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            change: SidecarChange::Reconcile {
                base,
                desired,
                transfer: false,
            },
        }
    }
    pub fn close_tab(tab: TabId) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            change: SidecarChange::CloseTab { tab },
        }
    }
    pub fn with_transfer(mut self) -> Self {
        if let SidecarChange::Reconcile { transfer, .. } = &mut self.change {
            *transfer = true;
        }
        self
    }
}

fn merge_tab(
    current: &mut Tab<ScopedPane>,
    base: Option<&Tab<ScopedPane>>,
    desired: &Tab<ScopedPane>,
) {
    let Some(base) = base else {
        // Two clients may concurrently discover the same tab. The second seed
        // contributes new leaves, but has no authority to erase existing ones.
        insert_missing(&mut current.root, &desired.root, None);
        if let Some(pip) = desired.presentation.pip() {
            current.set_pip_with_restore(
                Some(pip.pane()),
                desired.root.pane_ids().first().copied(),
                Some(pip.clone()),
            );
        }
        for entry in desired.presentation.parked() {
            current.park_with_restore(entry.pane(), Some(entry.clone()));
        }
        if desired.presentation.pip_geometry().is_some() {
            current
                .presentation
                .set_pip_geometry(desired.presentation.pip_geometry());
        }
        return;
    };
    if current == base {
        *current = desired.clone();
        return;
    }
    let old = base.root.pane_ids();
    let want = desired.root.pane_ids();
    for pane in old.iter().copied().filter(|pane| !want.contains(pane)) {
        // A client cannot remove the sole newer leaf it did not know about.
        if current.root.pane_ids().len() > 1 {
            current.root.remove_leaf(pane);
        }
    }
    // Unchanged leaves deleted by a newer authoritative edit stay deleted.
    insert_missing(&mut current.root, &desired.root, Some(&base.root));
    let common = |pane| old.contains(&pane) && want.contains(&pane);
    let old_common = base
        .root
        .filter_map_panes(&|pane| (common(pane) && current.root.contains(pane)).then_some(pane));
    let desired_common = desired
        .root
        .filter_map_panes(&|pane| (common(pane) && current.root.contains(pane)).then_some(pane));
    if old_common != desired_common {
        for pane in want.iter().copied().filter(|pane| common(*pane)) {
            if !current.root.contains(pane) {
                continue;
            }
            let (Some(before), Some(after)) = (
                base.root.relationship(pane),
                desired.root.relationship(pane),
            ) else {
                continue;
            };
            if before.axis() == after.axis()
                && before.first() == after.first()
                && before.siblings() == after.siblings()
            {
                if before.ratio() != after.ratio() {
                    set_ratio(&mut current.root, pane, after.axis(), after.ratio());
                }
                continue;
            }
            let Some(anchor) = after
                .siblings()
                .iter()
                .copied()
                .find(|id| current.root.contains(*id) && *id != pane)
            else {
                continue;
            };
            if let Some(now) = current.root.relationship(pane)
                && now.axis() == after.axis()
                && now.first() == after.first()
                && now.siblings().contains(&anchor)
            {
                continue;
            }
            if current.root.contains(pane) && current.root.pane_ids().len() > 1 {
                current.root.remove_leaf(pane);
            }
            current
                .root
                .split_leaf(anchor, pane, after.axis(), after.ratio(), after.first());
        }
    }
    current
        .presentation
        .remap_panes(|pane| current.root.contains(pane).then_some(pane));
    if desired.name != base.name {
        current.name = desired.name.clone();
    }
    if desired.group != base.group {
        current.group = desired.group;
    }
    if desired.last_auto != base.last_auto {
        current.last_auto = desired.last_auto.clone();
    }
    for entry in base.presentation.parked() {
        if !desired.presentation.is_parked(entry.pane())
            && current.presentation.is_parked(entry.pane())
        {
            current.restore_parked(entry.pane(), want.first().copied());
        }
    }
    for entry in desired.presentation.parked() {
        if !base.presentation.parked().contains(entry) {
            current.park_with_restore(entry.pane(), Some(entry.clone()));
        }
    }
    if desired.presentation.pip_geometry() != base.presentation.pip_geometry() {
        current
            .presentation
            .set_pip_geometry(desired.presentation.pip_geometry());
    }
    let before = base.presentation.pip().map(|pip| pip.pane());
    let after = desired.presentation.pip().map(|pip| pip.pane());
    if before != after {
        if after.is_some() || current.presentation.pip().map(|pip| pip.pane()) == before {
            current.set_pip_with_restore(
                after,
                want.first().copied(),
                desired.presentation.pip().cloned(),
            );
        }
    } else if desired.presentation != base.presentation
        && current.presentation.pip().map(|pip| pip.pane()) == before
    {
        // The pane identity is unchanged: only its original relationship changed.
        current.set_pip_with_restore(
            after,
            want.first().copied(),
            desired.presentation.pip().cloned(),
        );
    }
}

fn insert_missing(
    root: &mut PaneNode<ScopedPane>,
    desired: &PaneNode<ScopedPane>,
    base: Option<&PaneNode<ScopedPane>>,
) {
    for pane in desired.pane_ids() {
        if root.contains(pane) || base.is_some_and(|base| base.contains(pane)) {
            continue;
        }
        let Some(relation) = desired.relationship(pane) else {
            continue;
        };
        let anchor = relation
            .siblings()
            .iter()
            .copied()
            .find(|id| root.contains(*id))
            .or_else(|| root.pane_ids().first().copied());
        if let Some(anchor) = anchor {
            root.split_leaf(
                anchor,
                pane,
                relation.axis(),
                relation.ratio(),
                relation.first(),
            );
        }
    }
}

fn set_ratio(root: &mut PaneNode<ScopedPane>, pane: ScopedPane, axis: Axis, ratio: f32) -> bool {
    let PaneNode::Split {
        axis: split,
        ratio: current,
        a,
        b,
    } = root
    else {
        return false;
    };
    if *split == axis
        && (matches!(&**a, PaneNode::Leaf { pane: id } if *id == pane)
            || matches!(&**b, PaneNode::Leaf { pane: id } if *id == pane))
    {
        *current = ratio;
        return true;
    }
    set_ratio(a, pane, axis, ratio) || set_ratio(b, pane, axis, ratio)
}
