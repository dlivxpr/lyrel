//! Window-local synchronization authority. The machine mirror remains machine-owned.
//!
//! Transitions consume projections and rebuild outcomes, never a mutable workbench.
//! Queue acknowledgements and pulls are fenced by the same workspace epoch.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

// Forgetting and reopening the same workspace must not recycle an old task's token.
fn next_epoch() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

use super::desired::DesiredTab;
use super::reconcile::{SyncScope, WsMirror, diff, unaccounted};
use super::{Adopt, Arrival, ChosenName, ParkedOpen, Retiring};
use crate::core::group_key::WorkspaceGroups;
use crate::core::session::WorkspaceId;
use lyrel_core::core::machine::{LayoutDelta, PaneNode, Tab as TreeTab, TabId};
use lyrel_core::daemon::control::ControlRequest;

pub(super) enum SyncPhase {
    Unprimed { dirty: bool, priming: bool },
    Primed(WsMirror),
}

/// One window's right to publish a projection, not another machine-level tree.
pub(super) struct WindowSync {
    sync: SyncPhase,
    queue: VecDeque<ControlRequest>,
    inflight: bool,
    informed: bool,
    epoch: u64,
    rehydrate: Option<Adopt>,
    /// Baseline of the outstanding recovery; unchanged stale tabs do not overtake it.
    owed_over: Vec<TabId>,
    /// Local saves, unlike link/retry coordination, can overtake recovery in the same tab.
    recovery_edited: bool,
    /// Failed rebuilds remain outside deletion/reorder authority.
    not_rebuilt: Vec<TabId>,
    rehydrate_attempts: u32,
    then_open: Vec<ParkedOpen>,
    chosen_name: Option<ChosenName>,
    unsent_groups: Option<WorkspaceGroups>,
    retiring: Vec<Retiring>,
    closing: Vec<TabId>,
    retired: Vec<Retiring>,
    said_why_empty: bool,
}

impl Default for WindowSync {
    fn default() -> Self {
        Self {
            sync: SyncPhase::Unprimed { dirty: false, priming: false },
            queue: VecDeque::new(),
            inflight: false,
            informed: false,
            epoch: next_epoch(),
            rehydrate: None,
            owed_over: Vec::new(),
            recovery_edited: false,
            not_rebuilt: Vec::new(),
            rehydrate_attempts: 0,
            then_open: Vec::new(),
            chosen_name: None,
            unsent_groups: None,
            retiring: Vec::new(),
            closing: Vec::new(),
            retired: Vec::new(),
            said_why_empty: false,
        }
    }
}

pub(super) enum ProjectionResult {
    Prime,
    Waiting,
    Queued { tabs: Vec<TreeTab>, active: Option<TabId>, remembered: Vec<TabId> },
}

/// An issued batch owns its retirements even if its window is forgotten or replaced.
pub(super) struct Batch {
    pub epoch: u64,
    pub ops: Vec<ControlRequest>,
    pub retired: Vec<Retiring>,
}

pub(super) struct PrimeResult {
    pub dirty: bool,
    pub tabs: Vec<TreeTab>,
    pub active: Option<TabId>,
}

pub(super) struct HydrationResult {
    pub keep_window: bool,
    pub publish_window: bool,
    pub dirty: bool,
}

impl WindowSync {
    pub(super) fn record_local_write(&mut self) {
        if self.rehydrate.is_some()
            || matches!(self.sync, SyncPhase::Unprimed { priming: true, .. })
        {
            self.recovery_edited = true;
        }
    }

    pub(super) fn project(
        &mut self,
        client_ws: WorkspaceId,
        machine_ws: WorkspaceId,
        desired: &[DesiredTab],
        active: Option<TabId>,
        mut held: Vec<TabId>,
    ) -> ProjectionResult {
        let mirror = match &mut self.sync {
            SyncPhase::Unprimed { dirty, priming } => {
                *dirty = true;
                return if std::mem::replace(priming, true) {
                    ProjectionResult::Waiting
                } else {
                    ProjectionResult::Prime
                };
            }
            SyncPhase::Primed(mirror) => mirror,
        };
        let scope = if self.informed { SyncScope::Full } else { SyncScope::Additive };
        self.not_rebuilt.retain(|id| mirror.tabs.iter().any(|t| t.id == *id));
        held.extend(self.not_rebuilt.iter().copied());
        let mut missing = unaccounted(mirror, desired, &mut self.closing);
        missing.retain(|id| !held.contains(id));
        if scope == SyncScope::Full && !missing.is_empty() {
            log::warn!(
                "workspace {client_ws}: the window is not showing {} tab(s) the user did not \
                 close here; leaving them on the machine",
                missing.len()
            );
        }
        held.extend(missing);
        let mut ops = diff(machine_ws, mirror, desired, active, scope, &held);
        let sent = remember_closes(&mut ops, &mut self.retiring);
        let remembered = sent.iter().map(|r| r.tab).collect();
        self.retired.extend(sent);
        if ops.is_empty() {
            return ProjectionResult::Waiting;
        }
        self.queue.extend(ops);
        ProjectionResult::Queued { tabs: mirror.tabs.clone(), active: mirror.active, remembered }
    }

    pub(super) fn remembers_close(&self, tab: TabId) -> bool {
        self.informed && self.rehydrate.is_none()
            && matches!(&self.sync, SyncPhase::Primed(mirror) if mirror.tabs.iter().any(|t| t.id == tab))
    }

    pub(super) fn intend_close(&mut self, tab: TabId) {
        if !self.closing.contains(&tab) {
            self.closing.push(tab);
        }
    }

    pub(super) fn retire(&mut self, retiring: Retiring) {
        self.retiring.push(retiring);
    }

    pub(super) fn withdraw_retirement(&mut self, tab: TabId) -> Option<Retiring> {
        let at = self.retiring.iter().position(|r| r.tab == tab)?;
        Some(self.retiring.remove(at))
    }

    pub(super) fn is_settled(&self) -> bool {
        !self.inflight && self.queue.is_empty() && self.retiring.is_empty()
    }

    pub(super) fn is_disposable(&self) -> bool {
        self.informed && matches!(&self.sync, SyncPhase::Primed(mirror) if mirror.tabs.is_empty())
    }

    pub(super) fn inform(&mut self) {
        self.informed = true;
    }

    pub(super) fn match_tab(
        &self,
        id: TabId,
        known: &[TabId],
        panes: impl FnOnce() -> Vec<u64>,
    ) -> Option<TabId> {
        let SyncPhase::Primed(mirror) = &self.sync else { return None };
        if mirror.tabs.iter().any(|t| t.id == id) {
            return None;
        }
        let panes = panes();
        if panes.is_empty() {
            return None;
        }
        mirror.tabs.iter()
            .find(|t| !known.contains(&t.id) && panes.iter().any(|p| t.root.contains(*p)))
            .map(|t| t.id)
    }

    pub(super) fn preempt(&mut self) {
        self.sync = SyncPhase::Unprimed { dirty: false, priming: false };
        self.informed = false;
        self.epoch = next_epoch();
        self.inflight = false;
    }

    /// Withdraw queued closes, but never an issued batch's independently owned closes.
    pub(super) fn discard_queue(&mut self) -> (Vec<ControlRequest>, Vec<Retiring>) {
        (self.queue.drain(..).collect(), std::mem::take(&mut self.retired))
    }

    pub(super) fn begin_batch(&mut self) -> Option<Batch> {
        if self.inflight || self.queue.is_empty() {
            return None;
        }
        self.inflight = true;
        Some(Batch {
            epoch: self.epoch,
            ops: self.queue.drain(..).collect(),
            retired: std::mem::take(&mut self.retired),
        })
    }

    pub(super) fn has_batch_ready(&self) -> bool {
        !self.inflight && !self.queue.is_empty()
    }

    /// A completion from a retired daemon/request cannot alter its successor's queue.
    pub(super) fn finish_batch(&mut self, epoch: u64) -> bool {
        if self.epoch != epoch {
            return false;
        }
        self.inflight = false;
        true
    }

    pub(super) fn reprime(&mut self) {
        self.inflight = false;
        self.sync = SyncPhase::Unprimed { dirty: true, priming: true };
        self.epoch = next_epoch();
    }

    pub(super) fn park_name(&mut self, workspace: WorkspaceId, name: String) -> Result<(), String> {
        if matches!(self.sync, SyncPhase::Unprimed { .. }) {
            self.chosen_name = Some(ChosenName { workspace, name });
            Ok(())
        } else {
            Err(name)
        }
    }

    pub(super) fn chosen_name(&self, workspace: WorkspaceId) -> Option<String> {
        self.chosen_name.as_ref().filter(|n| n.workspace == workspace).map(|n| n.name.clone())
    }

    pub(super) fn settle_name(&mut self, workspace: WorkspaceId, arrival: Arrival) -> Option<String> {
        if self.chosen_name.as_ref().is_some_and(|n| n.workspace == workspace && arrival == Arrival::Created) {
            self.chosen_name.take().map(|n| n.name)
        } else {
            None
        }
    }

    pub(super) fn prime_unavailable(&mut self) {
        if let SyncPhase::Unprimed { priming, .. } = &mut self.sync {
            *priming = false;
        }
    }

    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(super) fn accepts_prime(&self, epoch: u64) -> bool {
        self.epoch == epoch && matches!(self.sync, SyncPhase::Unprimed { priming: true, .. })
    }

    pub(super) fn prime_failed(&mut self) {
        self.prime_unavailable();
    }

    pub(super) fn prime_landed(&mut self, mirror: WsMirror) -> PrimeResult {
        let dirty = matches!(self.sync, SyncPhase::Unprimed { dirty: true, .. });
        self.informed |= mirror.tabs.is_empty();
        self.rehydrate_attempts = 0;
        let result = PrimeResult { dirty, tabs: mirror.tabs.clone(), active: mirror.active };
        self.sync = SyncPhase::Primed(mirror);
        result
    }

    pub(super) fn push_groups(&mut self, workspace: WorkspaceId, groups: WorkspaceGroups) -> bool {
        match &mut self.sync {
            SyncPhase::Primed(mirror) => {
                if mirror.groups == groups { return false; }
                mirror.groups = groups.clone();
                self.queue.push_back(ControlRequest::WorkspaceSetGroups { workspace, groups });
                true
            }
            SyncPhase::Unprimed { .. } => {
                self.unsent_groups = Some(groups);
                false
            }
        }
    }

    /// A local edit wins over the pull and is queued before projected tab operations.
    pub(super) fn adopt_groups_from_pull(&mut self, workspace: WorkspaceId) -> bool {
        if let Some(unsent) = self.unsent_groups.take() {
            self.push_groups(workspace, unsent);
            return false;
        }
        matches!(self.sync, SyncPhase::Primed(_))
    }

    pub(super) fn groups(&self) -> Option<WorkspaceGroups> {
        match &self.sync {
            SyncPhase::Primed(mirror) => Some(mirror.groups.clone()),
            _ => None,
        }
    }

    pub(super) fn is_pending(&self) -> bool {
        matches!(self.sync, SyncPhase::Unprimed { priming: true, .. })
            || self.rehydrate.is_some() || !self.then_open.is_empty()
    }

    pub(super) fn park_open(&mut self, open: ParkedOpen) {
        if let ParkedOpen::Command { cwd, .. } = &open
            && let Some(folder) = self.then_open.iter_mut()
                .find(|p| matches!(p, ParkedOpen::Folder(path) if path == cwd))
        {
            *folder = open;
        } else {
            self.then_open.push(open);
        }
    }

    pub(super) fn take_parked(&mut self) -> Vec<ParkedOpen> {
        std::mem::take(&mut self.then_open)
    }

    pub(super) fn begin_hydration(&mut self, adopt: Adopt, showing: Vec<TabId>) -> (u64, u32) {
        self.sync = SyncPhase::Unprimed { dirty: false, priming: true };
        self.epoch = next_epoch();
        self.inflight = false;
        self.rehydrate = None;
        self.owed_over = showing;
        self.recovery_edited = false;
        if adopt == Adopt::Replace {
            self.informed = false;
        }
        (self.epoch, self.rehydrate_attempts)
    }

    pub(super) fn take_rehydrate(&mut self, showing: &[TabId]) -> Option<Adopt> {
        let adopt = self.rehydrate.take()?;
        if self.recovery_overtaken(adopt, showing) {
            self.rehydrate_attempts = 0;
            return None;
        }
        Some(adopt)
    }

    pub(super) fn recovery_overtaken(&self, adopt: Adopt, showing: &[TabId]) -> bool {
        adopt == Adopt::Replace
            && (self.recovery_edited || showing.iter().any(|id| !self.owed_over.contains(id)))
    }

    pub(super) fn owe_rehydration(&mut self, epoch: u64, adopt: Adopt) -> Option<u32> {
        if self.epoch != epoch { return None; }
        self.prime_unavailable();
        self.rehydrate = Some(adopt);
        self.rehydrate_attempts = self.rehydrate_attempts.saturating_add(1);
        Some(self.rehydrate_attempts)
    }

    pub(super) fn still_owed(&self, epoch: u64) -> bool {
        self.rehydrate.is_some() && self.epoch == epoch
    }

    pub(super) fn failures(&self) -> u32 {
        self.rehydrate_attempts
    }

    /// Publishing the pull's baseline does not yet grant failed rebuilds deletion rights.
    pub(super) fn hydration_landed(
        &mut self,
        mirror: WsMirror,
        adopt: Adopt,
        showing: &[TabId],
    ) -> HydrationResult {
        let dirty = matches!(self.sync, SyncPhase::Unprimed { dirty: true, .. });
        let overtaken = self.recovery_overtaken(adopt, showing);
        let empty = mirror.tabs.is_empty();
        let keep_window = overtaken || (adopt == Adopt::IfEmpty && !showing.is_empty());
        self.informed |= empty;
        self.sync = SyncPhase::Primed(mirror);
        self.rehydrate_attempts = 0;
        self.recovery_edited = false;
        self.said_why_empty = false;
        HydrationResult { keep_window, publish_window: dirty || empty || overtaken, dirty }
    }

    pub(super) fn explain_empty(&mut self) -> bool {
        !std::mem::replace(&mut self.said_why_empty, true)
    }

    pub(super) fn settle_rebuild(&mut self, wanted: usize, wanted_ids: &[TabId], showing: &[TabId]) -> usize {
        self.not_rebuilt = wanted_ids.iter().copied().filter(|id| !showing.contains(id)).collect();
        if !showing.is_empty() || wanted == 0 {
            self.informed = true;
        }
        self.not_rebuilt.len()
    }

    pub(super) fn apply_delta(&mut self, delta: &LayoutDelta) -> Option<bool> {
        match &mut self.sync {
            SyncPhase::Primed(mirror) => Some(apply_to_mirror(mirror, delta)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn prime_for_test(&mut self, tabs: Vec<TreeTab>) {
        self.sync = SyncPhase::Primed(WsMirror { tabs, ..Default::default() });
        self.informed = true;
        self.inflight = true;
    }

    #[cfg(test)]
    pub(super) fn queued_for_test(&self) -> Vec<ControlRequest> {
        self.queue.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(super) fn chosen_name_for_test(&self) -> Option<String> {
        self.chosen_name.as_ref().map(|n| n.name.clone())
    }
}

#[cfg(test)]
mod naming_tests;
#[cfg(test)]
mod recovery_tests;
/// Turn the sync's `TabClose` for each retiring tab into the close that asks
/// the machine to remember it, and answer the retirements that went out.
///
/// Done on the diff's output rather than inside it: the diff only knows that a
/// tab is gone from the window, which is the same whether it was closed, moved
/// to another window or never rebuilt. Only an explicit close registers a
/// retirement, so only that is remembered.
fn remember_closes(ops: &mut [ControlRequest], retiring: &mut Vec<Retiring>) -> Vec<Retiring> {
    let mut sent = Vec::new();
    for op in ops.iter_mut() {
        let ControlRequest::TabClose { workspace, tab } = *op else {
            continue;
        };
        let Some(at) = retiring.iter().position(|r| r.tab == tab) else {
            continue;
        };
        let retired = retiring.remove(at);
        *op = ControlRequest::TabCloseRemembered {
            workspace,
            tab,
            panes: retired.panes.clone(),
        };
        sent.push(retired);
    }
    sent
}
fn apply_to_mirror(mirror: &mut WsMirror, delta: &LayoutDelta) -> bool {
    match delta {
        // Nothing here is about a workspace's tab list, so the mirror is
        // already right. `WorkspaceDeleted` never reaches this far —
        // `on_layout_delta` hands it to `on_workspace_deleted` and returns —
        // and is listed only so a new delta cannot join this arm by accident.
        LayoutDelta::WorkspaceCreated { .. }
        | LayoutDelta::WorkspaceRenamed { .. }
        | LayoutDelta::WorkspaceTouched { .. }
        | LayoutDelta::WorkspaceDeleted
        | LayoutDelta::PaneFacts { .. } => true,
        LayoutDelta::ActiveTabChanged { tab } => {
            mirror.active = Some(*tab);
            true
        }
        LayoutDelta::TabCreated { at, tab } => {
            mirror.tabs.retain(|t| t.id != tab.id);
            let at = (*at).min(mirror.tabs.len());
            mirror.tabs.insert(at, tab.clone());
            true
        }
        LayoutDelta::TabClosed { tab, .. } => {
            let before = mirror.tabs.len();
            mirror.tabs.retain(|t| t.id != *tab);
            if mirror.tabs.is_empty() {
                mirror.active = None;
            }
            mirror.tabs.len() != before
        }
        LayoutDelta::TabRenamed { tab, name } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            t.name = name.clone();
            true
        }
        LayoutDelta::TabRegrouped {
            tab,
            group,
            last_auto,
        } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            t.group = *group;
            t.last_auto = last_auto.clone();
            true
        }
        LayoutDelta::GroupsChanged { groups } => {
            mirror.groups = groups.clone();
            true
        }
        LayoutDelta::TabMoved { tab, to } => {
            let Some(from) = mirror.tabs.iter().position(|t| t.id == *tab) else {
                return false;
            };
            let moved = mirror.tabs.remove(from);
            mirror.tabs.insert((*to).min(mirror.tabs.len()), moved);
            true
        }
        LayoutDelta::TabRestructured { tab, .. } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == tab.id) else {
                return false;
            };
            *t = tab.clone();
            true
        }
        LayoutDelta::RatioChanged { tab, path, ratio } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            match t.root.descend_mut(path) {
                Some(PaneNode::Split { ratio: r, .. }) => {
                    *r = *ratio;
                    true
                }
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod application_tests;

#[cfg(test)]
mod delta_tests;
#[cfg(test)]
mod retry_tests;
