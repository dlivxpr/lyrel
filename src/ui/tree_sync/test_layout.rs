use lyrel_core::core::machine::{Axis as TreeAxis, PaneSeed, TabId, TabPresentation};

use super::desired::{DesiredNode, DesiredTab};
use super::reconcile::WsMirror;

pub(super) fn seed(pane: u64) -> PaneSeed {
    PaneSeed {
        pane,
        cwd: Some(format!("/work/{pane}")),
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

pub(super) fn leaf(pane: u64) -> DesiredNode {
    DesiredNode::Leaf {
        pane,
        seed: seed(pane),
    }
}

pub(super) fn split(axis: TreeAxis, ratio: f32, a: DesiredNode, b: DesiredNode) -> DesiredNode {
    DesiredNode::Split {
        axis,
        ratio,
        a: Box::new(a),
        b: Box::new(b),
    }
}

pub(super) fn tab(id: TabId, root: DesiredNode) -> DesiredTab {
    DesiredTab {
        id,
        name: None,
        group: None,
        last_auto: None,
        root,
        presentation: TabPresentation::default(),
        presentation_fallback: None,
        hibernated: false,
    }
}

pub(super) fn assert_converged(mirror: &WsMirror, desired: &[DesiredTab]) {
    assert_eq!(mirror.tabs.len(), desired.len());
    for (m, d) in mirror.tabs.iter().zip(desired) {
        assert_eq!(m.id, d.id);
        assert_eq!(m.name, d.name);
        assert_eq!(m.group, d.group);
        assert_eq!(m.root, d.root.to_pane_node());
        assert_eq!(m.presentation, d.presentation);
    }
}
