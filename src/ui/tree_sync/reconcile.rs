//! Canonical ordered layout planner. Mutates the supplied optimistic mirror in place.
use lyrel_core::core::machine::{Axis as TreeAxis, PaneNode, Side, Tab as TreeTab, TabId, TabPresentation};
use lyrel_core::daemon::control::ControlRequest;

use crate::core::group_key::WorkspaceGroups;
use crate::core::session::WorkspaceId;

use super::desired::{DesiredNode, DesiredTab};
/// The machine's tabs a sync must leave alone: every one the window is not
/// showing that the user did not close here. `closing` is pruned on the way to
/// the intents the mirror still has a tab for — the rest have been carried out,
/// or were never the machine's to begin with.
pub(super) fn unaccounted(mirror: &WsMirror, desired: &[DesiredTab], closing: &mut Vec<TabId>) -> Vec<TabId> {
    closing.retain(|id| mirror.tabs.iter().any(|t| t.id == *id));
    mirror
        .tabs
        .iter()
        .map(|t| t.id)
        .filter(|id| !desired.iter().any(|d| d.id == *id) && !closing.contains(id))
        .collect()
}
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WsMirror {
    pub tabs: Vec<TreeTab>,
    pub active: Option<TabId>,
    pub groups: WorkspaceGroups,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SyncScope {
    Full,
    Additive,
}

pub(crate) fn diff(
    workspace: WorkspaceId,
    mirror: &mut WsMirror,
    desired: &[DesiredTab],
    desired_active: Option<TabId>,
    scope: SyncScope,
    held: &[TabId],
) -> Vec<ControlRequest> {
    let mut ops = Vec::new();

    if scope == SyncScope::Full {
        migrate_panes(workspace, mirror, desired, &mut ops);
        let mut index = 0;
        while index < mirror.tabs.len() {
            let id = mirror.tabs[index].id;
            if desired.iter().any(|t| t.id == id) || held.contains(&id) {
                index += 1;
                continue;
            }
            let closed = mirror.tabs.remove(index);
            ops.push(ControlRequest::TabClose {
                workspace,
                tab: closed.id,
            });
            heal_active(mirror, index);
        }
    }

    // The tabs the machine already has are settled first, and only then are the
    // new ones built. A pane that left its tab to become one of its own is in
    // both halves of that — the machine refuses to register a pane it already
    // holds, so the tab it left has to give it up before the tab it is
    // becoming can ask for it.
    for want in desired {
        if let Some(at) = mirror.tabs.iter().position(|t| t.id == want.id) {
            reconcile_presentation(workspace, &mut mirror.tabs[at], want, &mut ops);
            reconcile_tab(workspace, mirror, at, want, &mut ops);
            reconcile_presentation(workspace, &mut mirror.tabs[at], want, &mut ops);
        }
    }
    for (index, want) in desired.iter().enumerate() {
        if mirror.tabs.iter().any(|t| t.id == want.id) {
            continue;
        }
        let at = match scope {
            SyncScope::Full => index,
            SyncScope::Additive => mirror.tabs.len(),
        };
        create_tab(workspace, mirror, at, want, &mut ops);
    }

    if scope == SyncScope::Additive || !held.is_empty() {
        return ops;
    }

    for (index, want) in desired.iter().enumerate() {
        let at = mirror
            .tabs
            .iter()
            .position(|t| t.id == want.id)
            .expect("every desired tab exists after the passes above");
        if at != index {
            let tab = mirror.tabs.remove(at);
            mirror.tabs.insert(index, tab);
            ops.push(ControlRequest::TabMove {
                workspace,
                tab: want.id,
                to: index as u64,
            });
        }
    }

    if let Some(active) = desired_active
        && mirror.active != Some(active)
        && mirror.tabs.iter().any(|t| t.id == active)
    {
        mirror.active = Some(active);
        ops.push(ControlRequest::WorkspaceSetActiveTab {
            workspace,
            tab: active,
        });
    }

    ops
}

/// Carries panes across to the tab that now wants them, before anything else
/// gets a chance to read their old tab as one to close.
///
/// This is a tab dragged into another tab's layout: every pane it brought
/// keeps running and changes tab, and the tab it came from goes away once it
/// has nothing left. Told as `PaneMove`, which is the one op that can say that
/// — closing the old tab and building the new one would say instead that a
/// tab's worth of panes went away and a tab's worth arrived.
///
/// One move per round, because each move changes where the next one can go:
/// a pane may only land beside a pane its destination already holds, so a
/// two-pane tab crosses as its first pane and then the rest beside it. A round
/// that finds nothing left to do ends the pass, which is also what happens
/// when a move cannot be spelled this way at all — the passes below then treat
/// it as the reshape it is.
fn migrate_panes(
    workspace: WorkspaceId,
    mirror: &mut WsMirror,
    desired: &[DesiredTab],
    ops: &mut Vec<ControlRequest>,
) {
    while let Some((pane, to, axis, first)) = next_migration(mirror, desired) {
        let holders = |m: &WsMirror, p: u64| m.tabs.iter().position(|t| t.root.contains(p));
        let (Some(from), Some(dest)) = (holders(mirror, pane), holders(mirror, to)) else {
            return;
        };
        // The destination takes the pane before its old tab gives it up, so a
        // split that somehow will not take leaves the mirror as it was rather
        // than a pane short of the tree the daemon has.
        if !mirror.tabs[dest]
            .root
            .split_leaf(to, pane, axis, 0.5, first)
        {
            return;
        }
        // The rest of the bookkeeping `Machine::pane_move` does at the other
        // end: a tab that has just lost its last pane is gone, and the active
        // tab heals onto whatever took its place.
        if mirror.tabs[from].root.remove_leaf(pane).is_none() {
            mirror.tabs.remove(from);
            heal_active(mirror, from);
        }
        ops.push(ControlRequest::PaneMove {
            workspace,
            pane,
            to,
            axis,
            first,
        });
    }
}

/// The next pane sitting in a tab that no longer wants it, and the pane in the
/// tab that does that it can be put beside.
fn next_migration(mirror: &WsMirror, desired: &[DesiredTab]) -> Option<(u64, u64, TreeAxis, bool)> {
    for want in desired {
        let Some(at) = mirror.tabs.iter().position(|t| t.id == want.id) else {
            continue;
        };
        let root = want.root.to_pane_node();
        // Panes the machine has never heard of are splits, not moves: a side
        // holding one of those is not a side that can cross over.
        let arriving = |node: &PaneNode| {
            let ids = node.pane_ids();
            !ids.is_empty()
                && ids.iter().all(|p| {
                    mirror
                        .tabs
                        .iter()
                        .position(|t| t.root.contains(*p))
                        .is_some_and(|holder| holder != at)
                })
        };
        let settled = |node: &PaneNode| match node {
            PaneNode::Leaf { pane } => mirror.tabs[at].root.contains(*pane).then_some(*pane),
            _ => None,
        };
        if let Some(step) = arrival_site(&root, &arriving, &settled) {
            return Some(step);
        }
    }
    None
}

/// The split in the shape a tab wants where panes still living in another tab
/// meet a pane that is already here, read as a pane to move and the pane to
/// put it beside.
///
/// The side that is arriving may be a whole subtree — only its first pane
/// crosses on this round, and the ones behind it follow on later rounds, by
/// which time this same reading finds them the sites they want inside it. The
/// side that is staying has to be a single pane, because that is all a move can
/// split. Nothing else can be said in one move: a tab dropped against the outer
/// edge of a layout that is more than one pane deep has to go in above the
/// whole of it, and the passes after this one rebuild the tab instead.
fn arrival_site(
    node: &PaneNode,
    arriving: &impl Fn(&PaneNode) -> bool,
    settled: &impl Fn(&PaneNode) -> Option<u64>,
) -> Option<(u64, u64, TreeAxis, bool)> {
    let PaneNode::Split { axis, a, b, .. } = node else {
        return None;
    };
    let first = |side: &PaneNode| side.pane_ids().first().copied();
    if let (Some(to), true) = (settled(a), arriving(b)) {
        return Some((first(b)?, to, *axis, false));
    }
    if let (true, Some(to)) = (arriving(a), settled(b)) {
        return Some((first(a)?, to, *axis, true));
    }
    arrival_site(a, arriving, settled).or_else(|| arrival_site(b, arriving, settled))
}

fn heal_active(mirror: &mut WsMirror, removed: usize) {
    let named = mirror
        .active
        .is_some_and(|active| mirror.tabs.iter().any(|t| t.id == active));
    if named {
        return;
    }
    if mirror.tabs.is_empty() {
        mirror.active = None;
        return;
    }
    mirror.active = Some(mirror.tabs[removed.min(mirror.tabs.len() - 1)].id);
}

fn create_tab(
    workspace: WorkspaceId,
    mirror: &mut WsMirror,
    index: usize,
    want: &DesiredTab,
    ops: &mut Vec<ControlRequest>,
) {
    let (first, seed) = want.root.first_leaf();
    ops.push(ControlRequest::TabCreate {
        workspace,
        at: Some(index as u64),
        pane: seed.clone(),
        tab: Some(want.id),
    });
    let mut root = PaneNode::Leaf { pane: *first };
    materialize_splits(workspace, &want.root, &mut root, ops);
    if want.name.is_some() {
        ops.push(ControlRequest::TabRename {
            workspace,
            tab: want.id,
            name: want.name.clone(),
        });
    }
    if want.group.is_some() || want.last_auto.is_some() {
        ops.push(ControlRequest::TabSetGroup {
            workspace,
            tab: want.id,
            group: want.group,
            last_auto: want.last_auto.clone(),
        });
    }
    if want.hibernated {
        ops.push(ControlRequest::TabSetHibernated {
            workspace,
            tab: want.id,
            hibernated: true,
        });
    }
    mirror.tabs.insert(
        index.min(mirror.tabs.len()),
        TreeTab {
            id: want.id,
            name: want.name.clone(),
            group: want.group,
            last_auto: want.last_auto.clone(),
            root,
            hibernated: want.hibernated,
            presentation: TabPresentation::default(),
        },
    );
    let at = index.min(mirror.tabs.len().saturating_sub(1));
    reconcile_presentation(workspace, &mut mirror.tabs[at], want, ops);
    mirror.active = Some(want.id);
}

fn reconcile_presentation(
    workspace: WorkspaceId,
    tab: &mut TreeTab,
    want: &DesiredTab,
    ops: &mut Vec<ControlRequest>,
) {
    // Apply recovery before reconciling the tree, then replay records after
    // arbitrary reshaping. No parked leaf is ever interpreted as PaneClose.
    let removed = tab
        .presentation
        .parked()
        .iter()
        .filter(|entry| !want.presentation.is_parked(entry.pane()))
        .map(|entry| entry.pane())
        .collect::<Vec<_>>();
    for pane in removed {
        if tab.restore_parked(pane, want.presentation_fallback) {
            ops.push(ControlRequest::PaneRestore {
                workspace,
                pane,
                fallback: want.presentation_fallback,
            });
        }
    }
    for restore in want.presentation.parked() {
        if tab.park_with_restore(restore.pane(), Some(restore.clone())) {
            ops.push(ControlRequest::PanePark {
                workspace,
                pane: restore.pane(),
                restore: Some(restore.clone()),
            });
        }
    }
    let have = tab.presentation.pip().map(|pip| pip.pane());
    let desired = want.presentation.pip().map(|pip| pip.pane());
    if tab.presentation == want.presentation {
        return;
    }
    let fallback = tab.visible_root().and_then(|root| {
        have.and_then(|pane| want.root.sibling_of(pane))
            .filter(|pane| root.contains(*pane) && Some(*pane) != desired)
            .or_else(|| {
                want.presentation_fallback
                    .filter(|pane| root.contains(*pane) && Some(*pane) != desired)
            })
            .or_else(|| {
                root.pane_ids()
                    .into_iter()
                    .find(|pane| Some(*pane) != desired)
            })
    });
    let Some(pane) = desired.or(have) else {
        return;
    };
    let restore = want.presentation.pip().cloned();
    if tab.set_pip_with_restore(desired, fallback, restore.clone()) {
        ops.push(ControlRequest::PaneSetPresentation {
            workspace,
            pane,
            pip: desired.is_some(),
            fallback,
            restore,
        });
    }
    if tab
        .presentation
        .set_pip_geometry(want.presentation.pip_geometry())
        && desired.is_some()
    {
        ops.push(ControlRequest::PaneSetPipGeometry {
            workspace,
            pane,
            geometry: want.presentation.pip_geometry(),
        });
    }
}

fn materialize_splits(
    workspace: WorkspaceId,
    want: &DesiredNode,
    root: &mut PaneNode,
    ops: &mut Vec<ControlRequest>,
) {
    let DesiredNode::Split { axis, ratio, a, b } = want else {
        return;
    };
    let (anchor, _) = a.first_leaf();
    let (new, seed) = b.first_leaf();
    ops.push(ControlRequest::PaneSplit {
        workspace,
        pane: *anchor,
        axis: *axis,
        ratio: *ratio,
        new: seed.clone(),
        first: false,
    });
    root.split_leaf(*anchor, *new, *axis, *ratio, false);
    materialize_splits(workspace, a, root, ops);
    materialize_splits(workspace, b, root, ops);
}

fn reconcile_tab(
    workspace: WorkspaceId,
    mirror: &mut WsMirror,
    at: usize,
    want: &DesiredTab,
    ops: &mut Vec<ControlRequest>,
) {
    {
        let tab = &mut mirror.tabs[at];
        if tab.name != want.name {
            tab.name = want.name.clone();
            ops.push(ControlRequest::TabRename {
                workspace,
                tab: want.id,
                name: want.name.clone(),
            });
        }
        // One op for both: the hint rides with the membership, so a tab
        // whose repo changed costs one message, and the machine never holds
        // a hint and a group that came from two different moments.
        if tab.group != want.group || tab.last_auto != want.last_auto {
            tab.group = want.group;
            tab.last_auto = want.last_auto.clone();
            ops.push(ControlRequest::TabSetGroup {
                workspace,
                tab: want.id,
                group: want.group,
                last_auto: want.last_auto.clone(),
            });
        }
        // Ahead of anything structural. Going to sleep changes nothing else
        // about the tab; waking swaps every pane for its successor, which may
        // take a close-and-recreate below — and the mark has to come off the
        // tab that exists now, before that one is gone.
        if tab.hibernated != want.hibernated {
            tab.hibernated = want.hibernated;
            ops.push(ControlRequest::TabSetHibernated {
                workspace,
                tab: want.id,
                hibernated: want.hibernated,
            });
        }
    }

    let desired_root = want.root.to_pane_node();
    if mirror.tabs[at].root == desired_root {
        return;
    }
    if same_shape_and_panes(&mirror.tabs[at].root, &desired_root) {
        fix_ratios(
            workspace,
            want.id,
            &mut mirror.tabs[at].root,
            &desired_root,
            ops,
        );
        return;
    }

    let have = mirror.tabs[at].root.pane_ids();
    let wanted = desired_root.pane_ids();
    let added: Vec<u64> = wanted
        .iter()
        .copied()
        .filter(|p| !have.contains(p))
        .collect();
    let removed: Vec<u64> = have
        .iter()
        .copied()
        .filter(|p| !wanted.contains(p))
        .collect();

    let done = match (added.as_slice(), removed.as_slice()) {
        ([new], []) => try_single_split(workspace, mirror, at, want, &desired_root, *new, ops),
        ([], []) => try_single_move(workspace, mirror, at, &desired_root, ops),
        ([], gone) if !gone.is_empty() => {
            for pane in gone {
                mirror.tabs[at].root.remove_leaf(*pane);
                ops.push(ControlRequest::PaneClose {
                    workspace,
                    pane: *pane,
                });
            }
            same_shape_and_panes(&mirror.tabs[at].root, &desired_root)
        }
        ([new], [old]) => {
            let elsewhere = mirror
                .tabs
                .iter()
                .enumerate()
                .any(|(i, t)| i != at && t.root.contains(*new));
            let mut predicted = mirror.tabs[at].root.clone();
            predicted.replace_leaf(*old, *new);
            if !elsewhere && same_shape_and_panes(&predicted, &desired_root) {
                let seed = want
                    .root
                    .seed_of(*new)
                    .expect("the added pane is a desired leaf")
                    .clone();
                mirror.tabs[at].root = predicted;
                ops.push(ControlRequest::PaneReplace {
                    workspace,
                    old: *old,
                    new: seed,
                });
                true
            } else {
                false
            }
        }
        _ => false,
    };

    if done {
        fix_ratios(
            workspace,
            want.id,
            &mut mirror.tabs[at].root,
            &desired_root,
            ops,
        );
        return;
    }

    let closed = mirror.tabs.remove(at);
    ops.push(ControlRequest::TabClose {
        workspace,
        tab: closed.id,
    });
    heal_active(mirror, at);
    create_tab(workspace, mirror, at, want, ops);
}

fn try_single_split(
    workspace: WorkspaceId,
    mirror: &mut WsMirror,
    at: usize,
    want: &DesiredTab,
    desired_root: &PaneNode,
    new: u64,
    ops: &mut Vec<ControlRequest>,
) -> bool {
    let Some((sibling, axis, ratio, first)) = split_site(desired_root, new) else {
        return false;
    };
    let mut predicted = mirror.tabs[at].root.clone();
    if !predicted.split_leaf(sibling, new, axis, ratio, first) {
        return false;
    }
    if !same_shape_and_panes(&predicted, desired_root) {
        return false;
    }
    let seed = want
        .root
        .seed_of(new)
        .expect("the added pane is a desired leaf")
        .clone();
    mirror.tabs[at].root = predicted;
    ops.push(ControlRequest::PaneSplit {
        workspace,
        pane: sibling,
        axis,
        ratio,
        new: seed,
        first,
    });
    true
}

/// Reshapes a tab that still holds exactly the panes it did, when one pane
/// changing places accounts for the whole difference.
///
/// That is what dragging a pane across the layout is, and it is worth spotting:
/// the fallback for a reshape is to close the tab and build it again, which
/// tells every other reader of the machine that a tab went away and came back
/// when all that happened was a pane sliding sideways.
///
/// A swap of two panes that are not each other's siblings is not one move, and
/// still takes the fallback.
fn try_single_move(
    workspace: WorkspaceId,
    mirror: &mut WsMirror,
    at: usize,
    desired_root: &PaneNode,
    ops: &mut Vec<ControlRequest>,
) -> bool {
    for pane in mirror.tabs[at].root.pane_ids() {
        let Some((to, axis, _, first)) = split_site(desired_root, pane) else {
            continue;
        };
        let mut predicted = mirror.tabs[at].root.clone();
        if predicted.remove_leaf(pane) != Some(true) {
            continue;
        }
        // The daemon re-splits at a half whatever the wanted ratio is, so the
        // mirror has to predict that half; `fix_ratios` settles the rest.
        if !predicted.split_leaf(to, pane, axis, 0.5, first)
            || !same_shape_and_panes(&predicted, desired_root)
        {
            continue;
        }
        mirror.tabs[at].root = predicted;
        ops.push(ControlRequest::PaneMove {
            workspace,
            pane,
            to,
            axis,
            first,
        });
        return true;
    }
    false
}

fn split_site(node: &PaneNode, new: u64) -> Option<(u64, TreeAxis, f32, bool)> {
    let PaneNode::Split { axis, ratio, a, b } = node else {
        return None;
    };
    match (&**a, &**b) {
        (PaneNode::Leaf { pane }, sibling) if *pane == new => {
            if let PaneNode::Leaf { pane: s } = sibling {
                return Some((*s, *axis, *ratio, true));
            }
            return None;
        }
        (sibling, PaneNode::Leaf { pane }) if *pane == new => {
            if let PaneNode::Leaf { pane: s } = sibling {
                return Some((*s, *axis, *ratio, false));
            }
            return None;
        }
        _ => {}
    }
    if a.contains(new) {
        split_site(a, new)
    } else if b.contains(new) {
        split_site(b, new)
    } else {
        None
    }
}

fn same_shape_and_panes(a: &PaneNode, b: &PaneNode) -> bool {
    match (a, b) {
        (PaneNode::Leaf { pane: pa }, PaneNode::Leaf { pane: pb }) => pa == pb,
        (
            PaneNode::Split {
                axis: ax,
                a: aa,
                b: ab,
                ..
            },
            PaneNode::Split {
                axis: bx,
                a: ba,
                b: bb,
                ..
            },
        ) => ax == bx && same_shape_and_panes(aa, ba) && same_shape_and_panes(ab, bb),
        _ => false,
    }
}

fn fix_ratios(
    workspace: WorkspaceId,
    tab: TabId,
    mirror: &mut PaneNode,
    desired: &PaneNode,
    ops: &mut Vec<ControlRequest>,
) {
    fn walk(
        workspace: WorkspaceId,
        tab: TabId,
        mirror: &mut PaneNode,
        desired: &PaneNode,
        path: &mut Vec<Side>,
        ops: &mut Vec<ControlRequest>,
    ) {
        let (
            PaneNode::Split {
                ratio: mr,
                a: ma,
                b: mb,
                ..
            },
            PaneNode::Split {
                ratio: dr,
                a: da,
                b: db,
                ..
            },
        ) = (mirror, desired)
        else {
            return;
        };
        if (*mr - *dr).abs() > 1e-4 {
            *mr = *dr;
            ops.push(ControlRequest::PaneSetRatio {
                workspace,
                tab,
                path: path.clone(),
                ratio: *dr,
            });
        }
        path.push(Side::A);
        walk(workspace, tab, ma, da, path, ops);
        path.pop();
        path.push(Side::B);
        walk(workspace, tab, mb, db, path, ops);
        path.pop();
    }
    let mut path = Vec::new();
    walk(workspace, tab, mirror, desired, &mut path, ops);
}

#[cfg(test)]
mod operation_tests;
#[cfg(test)]
mod presentation_tests;
#[cfg(test)]
mod scope_tests;
#[cfg(test)]
mod hibernation_tests;
