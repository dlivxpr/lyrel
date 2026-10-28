//! Logical pane trees and pure tab presentation operations.

use serde::{Deserialize, Serialize};

use crate::core::group_key::{AutoKey, GroupId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(uuid::Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    A,
    B,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "K: Deserialize<'de>"))]
pub struct Tab<K = u64> {
    #[serde(default)]
    pub id: TabId,
    #[serde(default)]
    pub name: Option<String>,
    /// The pinned group this tab was put in, or `None` for one the sidebar
    /// files by itself. Only pinned groups are stored: an auto group is
    /// worked out from the tab's cwd every time it is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
    /// The auto group this tab last resolved to — a hint, not a membership.
    ///
    /// Auto groups are worked out from a repo probe, and at launch no probe
    /// has answered yet: without this every restored tab sat in Ungrouped
    /// until its own came back, then jumped. The GUI draws the tab here until
    /// the live answer lands, and the live answer always wins and rewrites
    /// it. It never outranks `group`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_auto: Option<AutoKey>,
    pub root: PaneNode<K>,
    /// Presentation never removes a pane from the logical tree.
    #[serde(default)]
    pub presentation: TabPresentation<K>,
    /// The tab was put to sleep: its panes were stopped to give their memory
    /// back, and it stays in the workspace to be woken later (#762).
    ///
    /// The panes keep their ids in `root` and their records in
    /// [`super::Machine::panes`] — cwd, shell, agent session — because waking is a
    /// restore of exactly those panes, through the same path a reboot takes.
    /// That is also what keeps the sweeps off them: a pane the tree still
    /// names is one whose stored screen and history are kept.
    ///
    /// Left out of the document while false, so a tree written by this build
    /// reads the same to one that predates the field.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hibernated: bool,
}

impl<K: Copy + Eq> Tab<K> {
    pub fn leaf(pane: K) -> Tab<K> {
        Tab {
            id: TabId::new(),
            name: None,
            group: None,
            last_auto: None,
            root: PaneNode::Leaf { pane },
            presentation: TabPresentation::default(),
            hibernated: false,
        }
    }

    pub fn map_panes<J: Copy + Eq>(&self, map: impl Fn(K) -> Option<J>) -> Option<Tab<J>> {
        Some(Tab {
            id: self.id,
            name: self.name.clone(),
            group: self.group,
            last_auto: self.last_auto.clone(),
            root: self.root.filter_map_panes(&map)?,
            presentation: self.presentation.map_panes(&map),
            hibernated: self.hibernated,
        })
    }

    /// Projects tiled panes without changing ownership, including an empty tile area.
    pub fn visible_root(&self) -> Option<PaneNode<K>> {
        self.root
            .project_visible(&|pane| !self.presentation.is_hidden(pane))
    }

    /// Changes the one PiP pane, restoring its predecessor before promotion.
    /// An invalid promotion is a no-op; `false` means nothing changed.
    pub fn set_pip(&mut self, pane: Option<K>, fallback: Option<K>) -> bool {
        if pane.is_some_and(|id| !self.root.contains(id) || self.presentation.is_parked(id))
            || self.presentation.pip().map(PipPane::pane) == pane
        {
            return false;
        }
        if let Some(previous) = self.presentation.pip.take() {
            self.restore_relationship(&previous, fallback);
        }
        self.presentation.pip = pane.map(|pane| self.root.pip_relationship(pane));
        true
    }

    /// Hides a live pane while retaining its original tiled relationship.
    pub fn park(&mut self, pane: K) -> bool {
        self.park_with_restore(pane, None)
    }

    /// Replays durable parking after rebuilding a logical tree, without
    /// capturing an intervening layout as the original recovery position.
    pub fn park_with_restore(&mut self, pane: K, restore: Option<PipPane<K>>) -> bool {
        if !self.root.contains(pane) || restore.as_ref().is_some_and(|entry| entry.pane != pane) {
            return false;
        }
        if let Some(index) = self
            .presentation
            .parked
            .iter()
            .position(|entry| entry.pane == pane)
        {
            if let Some(restore) = restore
                && self.presentation.parked[index] != restore
            {
                self.presentation.parked[index] = restore;
                return true;
            }
            return false;
        }
        let previous = if self
            .presentation
            .pip()
            .is_some_and(|entry| entry.pane == pane)
        {
            self.presentation.pip.take()
        } else {
            None
        };
        self.presentation.parked.push(
            restore
                .or(previous)
                .unwrap_or_else(|| self.root.pip_relationship(pane)),
        );
        true
    }

    /// Restores exactly one parked pane. Session creation is never a restore effect.
    pub fn restore_parked(&mut self, pane: K, fallback: Option<K>) -> bool {
        let Some(index) = self
            .presentation
            .parked
            .iter()
            .position(|entry| entry.pane == pane)
        else {
            return false;
        };
        let restore = self.presentation.parked.remove(index);
        self.restore_relationship(&restore, fallback);
        true
    }

    fn restore_relationship(&mut self, previous: &PipPane<K>, fallback: Option<K>) {
        // A formerly standalone pane has no split relationship to recover.
        // Keep any splits created while it was hidden instead of rebuilding
        // them from the singleton's placeholder axis and ratio.
        if !self.root.contains(previous.pane) || previous.siblings.is_empty() {
            return;
        }
        let current = self.root.pip_relationship(previous.pane);
        let original = previous
            .siblings
            .iter()
            .copied()
            .find(|id| self.root.contains(*id));
        let relationship_survives = current.axis == previous.axis
            && current.first == previous.first
            && previous
                .siblings
                .iter()
                .any(|id| current.siblings.contains(id));
        if relationship_survives {
            return;
        }
        let anchor = original.or_else(|| {
            fallback
                .filter(|id| {
                    *id != previous.pane
                        && self.root.contains(*id)
                        && !self.presentation.is_hidden(*id)
                })
                .or_else(|| {
                    self.visible_root()
                        .and_then(|root| root.first_other(previous.pane))
                })
                .or_else(|| self.root.first_other(previous.pane))
        });
        if let Some(anchor) = anchor {
            self.root.remove_leaf(previous.pane);
            self.root.split_leaf(
                anchor,
                previous.pane,
                previous.axis,
                previous.ratio,
                previous.first,
            );
        }
    }

    /// Applies synchronized presentation without recapturing its original
    /// sibling relationship after a logical-tree rebuild.
    pub fn set_pip_with_restore(
        &mut self,
        pane: Option<K>,
        fallback: Option<K>,
        restore: Option<PipPane<K>>,
    ) -> bool {
        if pane.is_some_and(|pane| !self.root.contains(pane) || self.presentation.is_parked(pane))
            || pane.is_some_and(|pane| restore.as_ref().is_some_and(|pip| pip.pane != pane))
        {
            return false;
        }
        if let (Some(pane), Some(restore)) = (pane, restore) {
            if self.presentation.pip().is_none_or(|pip| pip.pane() != pane) {
                self.set_pip(None, fallback);
            }
            if self.presentation.pip.as_ref() == Some(&restore) {
                return false;
            }
            self.presentation.pip = Some(restore);
            true
        } else {
            self.set_pip(pane, fallback)
        }
    }

    pub(in crate::core) fn forget_presentation(&mut self, pane: K) {
        if self.presentation.pip().is_some_and(|pip| pip.pane == pane) {
            self.presentation.pip = None;
        }
        self.presentation.parked.retain(|entry| entry.pane != pane);
    }

    pub(super) fn replace_pane(&mut self, old: K, new: K) {
        self.root.replace_leaf(old, new);
        self.presentation
            .remap_panes(|pane| Some(if pane == old { new } else { pane }));
    }
}

/// Durable tab presentation, independent of the complete logical pane tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "K: Deserialize<'de>"))]
pub struct TabPresentation<K = u64> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pip: Option<PipPane<K>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    parked: Vec<PipPane<K>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pip_geometry: Option<PipGeometry>,
}

impl<K> Default for TabPresentation<K> {
    fn default() -> Self {
        Self {
            pip: None,
            parked: Vec::new(),
            pip_geometry: None,
        }
    }
}

impl<K: Copy + Eq> TabPresentation<K> {
    pub fn pip(&self) -> Option<&PipPane<K>> {
        self.pip.as_ref()
    }

    pub fn map_panes<J: Copy + Eq>(&self, map: impl Fn(K) -> Option<J>) -> TabPresentation<J> {
        TabPresentation {
            pip: self.pip.as_ref().and_then(|pip| pip.map_panes(&map)),
            parked: self
                .parked
                .iter()
                .filter_map(|entry| entry.map_panes(&map))
                .collect(),
            pip_geometry: self.pip_geometry,
        }
    }

    pub fn parked(&self) -> &[PipPane<K>] {
        &self.parked
    }

    pub fn is_parked(&self, pane: K) -> bool {
        self.parked.iter().any(|entry| entry.pane == pane)
    }

    pub fn is_hidden(&self, pane: K) -> bool {
        self.is_parked(pane) || self.pip().is_some_and(|entry| entry.pane == pane)
    }

    pub fn pip_geometry(&self) -> Option<PipGeometry> {
        self.pip_geometry
    }

    pub fn set_pip_geometry(&mut self, geometry: Option<PipGeometry>) -> bool {
        if self.pip_geometry == geometry {
            return false;
        }
        self.pip_geometry = geometry;
        true
    }

    /// Rebinds restored identities without discarding the original relationship.
    /// A missing PiP target clears presentation. Missing siblings retain their
    /// old, absent IDs so restoration can detect that every sibling was lost.
    pub fn remap_panes(&mut self, map: impl Fn(K) -> Option<K>) {
        fn remap<K: Copy>(entry: &mut PipPane<K>, map: &impl Fn(K) -> Option<K>) -> bool {
            let Some(pane) = map(entry.pane) else {
                return false;
            };
            entry.pane = pane;
            for sibling in &mut entry.siblings {
                if let Some(pane) = map(*sibling) {
                    *sibling = pane;
                }
            }
            true
        }
        if let Some(pip) = &mut self.pip
            && !remap(pip, &map)
        {
            self.pip = None;
        }
        self.parked.retain_mut(|entry| remap(entry, &map));
    }
}

/// Shared placement intent, not a client window rectangle. Position is a
/// fraction of available travel; size is a fraction of the content viewport.
/// Each client resolves terminal minima and shadow clearance at its own scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PipGeometry {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Default for PipGeometry {
    fn default() -> Self {
        Self {
            x: 1.,
            y: 1.,
            width: 0.42,
            height: 0.42,
        }
    }
}

impl PipGeometry {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Option<Self> {
        let valid = [x, y, width, height]
            .into_iter()
            .all(|v| v.is_finite() && (0. ..=1.).contains(&v));
        (valid && width > 0. && height > 0.).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }
    pub fn x(self) -> f32 {
        self.x
    }
    pub fn y(self) -> f32 {
        self.y
    }
    pub fn width(self) -> f32 {
        self.width
    }
    pub fn height(self) -> f32 {
        self.height
    }
    pub fn is_valid(self) -> bool {
        Self::new(self.x, self.y, self.width, self.height).is_some()
    }
}

/// One live pane presented outside the tiled projection, with its original
/// sibling relationship rather than an obsolete snapshot of the whole tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "K: Deserialize<'de>"))]
pub struct PipPane<K = u64> {
    pane: K,
    siblings: Vec<K>,
    axis: Axis,
    ratio: f32,
    first: bool,
}

impl<K: Copy> PipPane<K> {
    pub fn pane(&self) -> K {
        self.pane
    }

    pub fn siblings(&self) -> &[K] {
        &self.siblings
    }
    pub fn axis(&self) -> Axis {
        self.axis
    }
    pub fn ratio(&self) -> f32 {
        self.ratio
    }
    pub fn first(&self) -> bool {
        self.first
    }

    pub fn map_panes<J: Copy>(&self, map: impl Fn(K) -> Option<J>) -> Option<PipPane<J>> {
        Some(PipPane {
            pane: map(self.pane)?,
            siblings: self.siblings.iter().filter_map(|pane| map(*pane)).collect(),
            axis: self.axis,
            ratio: self.ratio,
            first: self.first,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "K: Deserialize<'de>"))]
pub enum PaneNode<K = u64> {
    Leaf {
        pane: K,
    },
    Split {
        axis: Axis,
        #[serde(default = "default_ratio")]
        ratio: f32,
        a: Box<PaneNode<K>>,
        b: Box<PaneNode<K>>,
    },
}

fn default_ratio() -> f32 {
    0.5
}

impl<K: Copy + Eq> PaneNode<K> {
    pub fn filter_map_panes<J: Copy + Eq>(
        &self,
        map: &impl Fn(K) -> Option<J>,
    ) -> Option<PaneNode<J>> {
        match self {
            Self::Leaf { pane } => map(*pane).map(|pane| PaneNode::Leaf { pane }),
            Self::Split { axis, ratio, a, b } => {
                match (a.filter_map_panes(map), b.filter_map_panes(map)) {
                    (Some(a), Some(b)) => Some(PaneNode::Split {
                        axis: *axis,
                        ratio: *ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (a, b) => a.or(b),
                }
            }
        }
    }

    pub fn relationship(&self, pane: K) -> Option<PipPane<K>> {
        self.contains(pane).then(|| self.pip_relationship(pane))
    }

    fn project_visible(&self, visible: &impl Fn(K) -> bool) -> Option<Self> {
        match self {
            Self::Leaf { pane } => visible(*pane).then_some(Self::Leaf { pane: *pane }),
            Self::Split { axis, ratio, a, b } => {
                match (a.project_visible(visible), b.project_visible(visible)) {
                    (Some(a), Some(b)) => Some(Self::Split {
                        axis: *axis,
                        ratio: *ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (a, b) => a.or(b),
                }
            }
        }
    }

    fn first_other(&self, excluded: K) -> Option<K> {
        match self {
            Self::Leaf { pane } => (*pane != excluded).then_some(*pane),
            Self::Split { a, b, .. } => a.first_other(excluded).or_else(|| b.first_other(excluded)),
        }
    }

    fn pip_relationship(&self, pane: K) -> PipPane<K> {
        match self {
            Self::Split { axis, ratio, a, b } => {
                let (target, sibling, first) = if a.contains(pane) {
                    (a, b, true)
                } else {
                    (b, a, false)
                };
                if matches!(&**target, Self::Leaf { .. }) {
                    PipPane {
                        pane,
                        siblings: sibling.pane_ids(),
                        axis: *axis,
                        ratio: *ratio,
                        first,
                    }
                } else {
                    target.pip_relationship(pane)
                }
            }
            Self::Leaf { .. } => PipPane {
                pane,
                siblings: Vec::new(),
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: false,
            },
        }
    }
    pub fn pane_ids(&self) -> Vec<K> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<K>) {
        match self {
            PaneNode::Leaf { pane } => out.push(*pane),
            PaneNode::Split { a, b, .. } => {
                a.collect_panes(out);
                b.collect_panes(out);
            }
        }
    }

    pub fn contains(&self, pane: K) -> bool {
        match self {
            PaneNode::Leaf { pane: p } => *p == pane,
            PaneNode::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    pub fn descend_mut(&mut self, path: &[Side]) -> Option<&mut PaneNode<K>> {
        match path.split_first() {
            None => Some(self),
            Some((side, rest)) => match self {
                PaneNode::Leaf { .. } => None,
                PaneNode::Split { a, b, .. } => match side {
                    Side::A => a.descend_mut(rest),
                    Side::B => b.descend_mut(rest),
                },
            },
        }
    }

    pub fn split_leaf(&mut self, pane: K, new: K, axis: Axis, ratio: f32, first: bool) -> bool {
        match self {
            PaneNode::Leaf { pane: p } if *p == pane => {
                let old = PaneNode::Leaf { pane };
                let added = PaneNode::Leaf { pane: new };
                let (a, b) = if first { (added, old) } else { (old, added) };
                *self = PaneNode::Split {
                    axis,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                };
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { a, b, .. } => {
                a.split_leaf(pane, new, axis, ratio, first)
                    || b.split_leaf(pane, new, axis, ratio, first)
            }
        }
    }

    pub fn remove_leaf(&mut self, pane: K) -> Option<bool> {
        match self {
            PaneNode::Leaf { pane: p } => {
                if *p == pane {
                    None
                } else {
                    Some(false)
                }
            }
            PaneNode::Split { a, b, .. } => {
                if matches!(&**a, PaneNode::Leaf { pane: p } if *p == pane) {
                    *self = (**b).clone();
                    return Some(true);
                }
                if matches!(&**b, PaneNode::Leaf { pane: p } if *p == pane) {
                    *self = (**a).clone();
                    return Some(true);
                }
                match a.remove_leaf(pane) {
                    Some(true) => Some(true),
                    Some(false) => b.remove_leaf(pane),
                    None => Some(false),
                }
            }
        }
    }

    pub fn replace_leaf(&mut self, old: K, new: K) -> bool {
        match self {
            PaneNode::Leaf { pane } if *pane == old => {
                *pane = new;
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { a, b, .. } => a.replace_leaf(old, new) || b.replace_leaf(old, new),
        }
    }
}
