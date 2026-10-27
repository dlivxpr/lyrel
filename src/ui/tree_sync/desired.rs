//! Desired layout values shared by GUI projection and ordered reconciliation.
use crate::core::group_key::{AutoKey, GroupId};
use lyrel_core::core::machine::{
    Axis as TreeAxis, PaneNode, PaneRecord, PaneSeed, TabId, TabPresentation,
};
#[derive(Debug, Clone)]
pub(crate) struct DesiredTab {
    pub id: TabId,
    pub name: Option<String>,
    pub group: Option<GroupId>,
    pub last_auto: Option<AutoKey>,
    pub root: DesiredNode,
    pub presentation: TabPresentation,
    pub presentation_fallback: Option<u64>,
    /// The tab is asleep, and `root` is the layout it will wake into — the
    /// same pane ids the machine already holds, none of them running.
    pub hibernated: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum DesiredNode {
    Leaf {
        pane: u64,
        seed: PaneSeed,
    },
    Split {
        axis: TreeAxis,
        ratio: f32,
        a: Box<DesiredNode>,
        b: Box<DesiredNode>,
    },
}

impl DesiredNode {
    pub(super) fn first_leaf(&self) -> (&u64, &PaneSeed) {
        match self {
            DesiredNode::Leaf { pane, seed } => (pane, seed),
            DesiredNode::Split { a, .. } => a.first_leaf(),
        }
    }

    pub(super) fn to_pane_node(&self) -> PaneNode {
        match self {
            DesiredNode::Leaf { pane, .. } => PaneNode::Leaf { pane: *pane },
            DesiredNode::Split { axis, ratio, a, b } => PaneNode::Split {
                axis: *axis,
                ratio: *ratio,
                a: Box::new(a.to_pane_node()),
                b: Box::new(b.to_pane_node()),
            },
        }
    }

    pub(super) fn seed_of(&self, pane: u64) -> Option<&PaneSeed> {
        match self {
            DesiredNode::Leaf { pane: p, seed } => (*p == pane).then_some(seed),
            DesiredNode::Split { a, b, .. } => a.seed_of(pane).or_else(|| b.seed_of(pane)),
        }
    }

    pub(super) fn sibling_of(&self, pane: u64) -> Option<u64> {
        let DesiredNode::Split { a, b, .. } = self else {
            return None;
        };
        match (a.as_ref(), b.as_ref()) {
            (DesiredNode::Leaf { pane: id, .. }, _) if *id == pane => Some(*b.first_leaf().0),
            (_, DesiredNode::Leaf { pane: id, .. }) if *id == pane => Some(*a.first_leaf().0),
            _ => a.sibling_of(pane).or_else(|| b.sibling_of(pane)),
        }
    }
}
/// The records the machine's `register_pane` will mint from the seeds a sync
/// pushes, for the window to mirror itself — they come back to it in nothing
/// it is sent (#612). Its own terminals stand in for the machine's liveness
/// probe: a pane this window holds open is one that registry answers alive, and
/// one still connecting is not yet anyone's to call — later facts settle it.
pub(super) fn seeded_records(desired: &[DesiredTab], live: impl Fn(u64) -> bool) -> Vec<PaneRecord> {
    fn walk(node: &DesiredNode, live: &impl Fn(u64) -> bool, out: &mut Vec<PaneRecord>) {
        match node {
            DesiredNode::Leaf { pane, seed } => out.push(seed.clone().into_record(live(*pane))),
            DesiredNode::Split { a, b, .. } => {
                walk(a, live, out);
                walk(b, live, out);
            }
        }
    }
    let mut out = Vec::new();
    for tab in desired {
        walk(&tab.root, &live, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tree_sync::test_layout::{leaf, split, tab};
    #[test]
    fn seeded_records_carry_the_seed_and_the_windows_own_liveness() {
        let desired = vec![
            tab(
                TabId::new(),
                split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)),
            ),
            tab(TabId::new(), leaf(3)),
        ];
        let records = seeded_records(&desired, |pane| pane != 2);
        assert_eq!(
            records.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(records[0].cwd.as_deref(), Some("/work/1"));
        assert!(records[0].live, "a pane the window holds open is live");
        assert!(!records[1].live, "one still connecting is not yet");
        assert!(records[2].live);
    }

}
