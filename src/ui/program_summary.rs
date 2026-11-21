//! Workbench presentation of daemon-authored program records.
//!
//! A tab's summary covers every logical leaf it owns — painted, hidden or
//! parked — and is read from the authoritative projection each terminal
//! already holds. State is never inferred from output, titles, glyphs or
//! process names, and the versions #314 publishes stay separate from the
//! recency that breaks a tie here.
//!
//! The summary is cached behind a cheap per-leaf key: the pane's session,
//! semantic/result counters and current representative. Rendering compares
//! that key, so a frame that leaves membership and versions alone reuses the
//! previous value; an applied frame refreshes the presentation from the
//! bounded current projection, because daemon recency can move without either
//! counter. Ordinary output never clones a record set or rebuilds a tooltip.

use crate::ui::program_status::record_label;
use crate::ui::tab::Tab;
#[cfg(test)]
use gpui_kit::Entity;
use gpui_kit::{App, EntityId, SharedString, Window};
#[cfg(test)]
use lyrel_core::core::program_status::{
    ProgramRecord, ProgramState, ProgramStatusClient, plain_program_text,
};
#[cfg(test)]
use lyrel_terminal::view::TerminalView;
#[cfg(test)]
use lyrel_ui_base::i18n::{L10nKey, t_fmt};
#[cfg(test)]
use std::rc::Rc;

/// How many records besides the representative the hover card lists.
///
/// A pane may hold up to 256 records; a card that enumerated every one would
/// be neither readable nor worth laying out on hover. The count of what stayed
/// out is still shown.
#[cfg(test)]
const TOOLTIP_RECORDS: usize = 6;

/// A current record, with the pane whose daemon produced it.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ProgramEntry {
    pane_id: u64,
    record: ProgramRecord,
}

#[cfg(test)]
impl ProgramEntry {
    /// The daemon pane id this record belongs to, as the notifications and
    /// close prompts already spell panes (`#3`).
    pub(crate) fn pane_id(&self) -> u64 {
        self.pane_id
    }

    pub(crate) fn record(&self) -> &ProgramRecord {
        &self.record
    }

    /// The line the hover card shows: pane, record id when the program named
    /// itself, then the record's own readable label.
    pub(crate) fn tooltip_label(&self) -> String {
        let mut label = format!("#{}", self.pane_id());
        if !self.record.id().is_empty() {
            label.push_str(" · ");
            label.push_str(&t_fmt(
                L10nKey::ProgramRecordId,
                &[("id", &plain_program_text(self.record.id()))],
            ));
        }
        label.push_str(" · ");
        label.push_str(&record_label(&self.record));
        label
    }

    /// Whether two entries would draw the same line in the same place.
    ///
    /// Recency and both counters are deliberately absent: they select and
    /// order entries, but a recency refresh that changes neither selection nor
    /// order changes no pixel and must not earn a repaint.
    fn shows_same(&self, other: &Self) -> bool {
        self.pane_id == other.pane_id
            && self.record.id() == other.record.id()
            && self.record.state() == other.record.state()
            && self.record.kind() == other.record.kind()
            && self.record.app() == other.record.app()
            && self.record.title() == other.record.title()
            && self.record.message() == other.record.message()
            && self.record.progress() == other.record.progress()
    }
}

/// The one program state a tab presents, plus the current records a hover
/// card can name after it.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ProgramSummary {
    representative: ProgramEntry,
    others: Vec<ProgramEntry>,
    /// Records beyond the representative and `others`, not enumerated.
    hidden: usize,
}

/// Two summaries are the same presentation when every drawn line and its
/// order match. Versions and recency are selection inputs, not text: a frame
/// that only refreshes them is not a reason to paint.
#[cfg(test)]
impl PartialEq for ProgramSummary {
    fn eq(&self, other: &Self) -> bool {
        self.hidden == other.hidden
            && self.representative.shows_same(&other.representative)
            && self.others.len() == other.others.len()
            && self
                .others
                .iter()
                .zip(&other.others)
                .all(|(left, right)| left.shows_same(right))
    }
}

#[cfg(test)]
impl Eq for ProgramSummary {}

#[cfg(test)]
impl ProgramSummary {
    pub(crate) fn representative(&self) -> &ProgramEntry {
        &self.representative
    }

    /// The other current records, most urgent first, bounded for a hover card.
    pub(crate) fn others(&self) -> &[ProgramEntry] {
        &self.others
    }

    pub(crate) fn hidden(&self) -> usize {
        self.hidden
    }
}

/// One leaf's program projection, reduced to what can tell two frames apart.
///
/// Session identity and both counters are stored verbatim — never rebuilt from
/// payload hashes — and the representative's id and recency make an
/// identical-refresh tie change visible without a version bump.
#[cfg(test)]
struct LeafKey {
    leaf: EntityId,
    session: Option<String>,
    semantic: u64,
    result: u64,
    records: usize,
    representative: Option<(String, u64)>,
}

#[cfg(test)]
impl LeafKey {
    fn of(leaf: EntityId, status: &ProgramStatusClient) -> Self {
        Self {
            leaf,
            session: status.session_id().map(str::to_owned),
            semantic: status.semantic_version(),
            result: status.result_version(),
            records: status.records().count(),
            representative: status
                .representative()
                .map(|record| (record.id().to_owned(), record.updated())),
        }
    }

    /// Compared against the live projection without allocating a new key.
    fn matches(&self, leaf: &Entity<TerminalView>, cx: &App) -> bool {
        if leaf.entity_id() != self.leaf {
            return false;
        }
        leaf.read(cx).with_program_status(|status| {
            status.session_id() == self.session.as_deref()
                && status.semantic_version() == self.semantic
                && status.result_version() == self.result
                && status.records().count() == self.records
                && match (status.representative(), self.representative.as_ref()) {
                    (None, None) => true,
                    (Some(record), Some((id, updated))) => {
                        record.id() == id.as_str() && record.updated() == *updated
                    }
                    _ => false,
                }
        })
    }
}

/// The tab summary the last refresh produced, and what it was produced from.
#[derive(Default)]
pub(crate) struct ProgramSummaryCache {
    #[cfg(test)]
    keys: Vec<LeafKey>,
    #[cfg(test)]
    summary: Option<Rc<ProgramSummary>>,
    current: CurrentProgram,
}

/// What the active pane's status bar last presented.
///
/// The bar reads the representative live; this cache only decides whether a
/// frame would change that presentation, so a record another pane outranks
/// cannot silently leave the bar stale.
#[derive(Default)]
struct CurrentProgram {
    leaf: Option<EntityId>,
    label: Option<SharedString>,
}

impl ProgramSummaryCache {
    /// The current summary, rebuilt only when a leaf's projection moved.
    #[cfg(test)]
    fn summary(&mut self, leaves: &[Entity<TerminalView>], cx: &App) -> Option<Rc<ProgramSummary>> {
        if !self.matches(leaves, cx) {
            let (keys, next) = collect(leaves, cx);
            self.keys = keys;
            self.summary = next.map(Rc::new);
        }
        self.summary.clone()
    }

    /// Rebuild after an authoritative frame and report whether the presented
    /// summary actually changed.
    ///
    /// Every applied frame can move a record's daemon recency without touching
    /// either counter — an identical report still keeps the cross-pane tie
    /// honest — so an event always re-reads the bounded current projection.
    /// What is suppressed is the repaint: an update another pane outranks, or
    /// one whose record order and text are unchanged, leaves the previous
    /// value in place and returns `false`.
    #[cfg(test)]
    fn refresh(&mut self, leaves: &[Entity<TerminalView>], cx: &App) -> bool {
        let (keys, next) = collect(leaves, cx);
        self.keys = keys;
        let changed = self.summary.as_deref() != next.as_ref();
        if changed {
            self.summary = next.map(Rc::new);
        }
        changed
    }

    /// Adopt the active pane's current representative and report whether what
    /// the status bar shows changed. A status bar that moved to another pane
    /// has already painted for it; adopting that pane here is bookkeeping.
    fn current_label_changed(
        &mut self,
        leaf: Option<EntityId>,
        label: Option<SharedString>,
    ) -> bool {
        let changed = match (self.current.leaf, leaf) {
            (Some(cached), Some(current)) if cached == current => self.current.label != label,
            (None, Some(_)) => label.is_some(),
            _ => false,
        };
        self.current.leaf = leaf;
        self.current.label = label;
        changed
    }

    #[cfg(test)]
    fn matches(&self, leaves: &[Entity<TerminalView>], cx: &App) -> bool {
        leaves.len() == self.keys.len()
            && leaves
                .iter()
                .zip(&self.keys)
                .all(|(leaf, key)| key.matches(leaf, cx))
    }
}

impl Tab {
    /// This tab's program summary, read from every logical leaf it owns.
    ///
    /// Reading the leaves themselves is what keeps parked and hidden panes in
    /// the answer, and a closed, moved or restored pane is simply a different
    /// set of leaves on the next read. The cache only pays for a rebuild when
    /// that set's authoritative projections differ.
    #[cfg(test)]
    pub(crate) fn program_summary(&self, cx: &App) -> Option<Rc<ProgramSummary>> {
        let leaves = self.pane.terminals();
        self.program_summary_cache.borrow_mut().summary(&leaves, cx)
    }

    /// Recompute after a program-status frame; `true` when the visible summary
    /// changed and the window has a reason to paint again.
    #[cfg(test)]
    pub(crate) fn refresh_program_summary(&self, cx: &App) -> bool {
        let leaves = self.pane.terminals();
        self.program_summary_cache.borrow_mut().refresh(&leaves, cx)
    }

    /// `true` when a frame changed what the active pane's status bar shows.
    ///
    /// Samples the same Context Pane the bar renders — the last valid
    /// terminal pane — so a frame that only moved that pane's representative
    /// is what asks for a repaint (#349).
    pub(crate) fn refresh_current_program_status(&self, window: &Window, cx: &App) -> bool {
        // `detail_pane` is the per-tab half of the app's Context Pane
        // resolution — focused leaf, else the leaf the tab last came back to.
        let leaf = self.detail_pane(window, cx);
        let (leaf_id, label) = match leaf {
            Some(leaf) => {
                let id = leaf.entity_id();
                let label = leaf
                    .read(cx)
                    .with_program_status(|status| status.representative().map(record_label));
                (Some(id), label.map(SharedString::from))
            }
            None => (None, None),
        };
        self.program_summary_cache
            .borrow_mut()
            .current_label_changed(leaf_id, label)
    }
}

#[cfg(test)]
fn collect(leaves: &[Entity<TerminalView>], cx: &App) -> (Vec<LeafKey>, Option<ProgramSummary>) {
    let mut keys = Vec::with_capacity(leaves.len());
    let mut entries = Vec::with_capacity(TOOLTIP_RECORDS + 1);
    let mut count = 0usize;
    for leaf in leaves {
        let view = leaf.read(cx);
        let pane_id = view.pane_id;
        // Select each pane's bounded leaders by reference under its existing
        // lock. Records that cannot appear in the card are never cloned.
        view.with_program_status(|status| {
            keys.push(LeafKey::of(leaf.entity_id(), status));
            let mut leaders: [Option<&ProgramRecord>; TOOLTIP_RECORDS + 1] =
                [None; TOOLTIP_RECORDS + 1];
            for record in status.records() {
                count += 1;
                let position = leaders.iter().position(|candidate| {
                    candidate.is_none_or(|candidate| {
                        record_order(record, pane_id, candidate, pane_id).is_lt()
                    })
                });
                if let Some(position) = position {
                    leaders.copy_within(position..TOOLTIP_RECORDS, position + 1);
                    leaders[position] = Some(record);
                }
            }
            for record in leaders.into_iter().flatten() {
                let position = entries
                    .iter()
                    .position(|candidate: &ProgramEntry| {
                        record_order(record, pane_id, &candidate.record, candidate.pane_id).is_lt()
                    })
                    .unwrap_or(entries.len());
                if position <= TOOLTIP_RECORDS {
                    if entries.len() > TOOLTIP_RECORDS {
                        entries.pop();
                    }
                    entries.insert(
                        position,
                        ProgramEntry {
                            pane_id,
                            record: record.clone(),
                        },
                    );
                }
            }
        });
    }
    let mut entries = entries.into_iter();
    let summary = entries.next().map(|representative| ProgramSummary {
        representative,
        others: entries.collect(),
        hidden: count.saturating_sub(TOOLTIP_RECORDS + 1),
    });
    (keys, summary)
}

#[cfg(test)]
fn urgency(record: &ProgramRecord) -> u8 {
    match record.state() {
        ProgramState::Blocked => 4,
        ProgramState::Error => 3,
        ProgramState::Working => 2,
        ProgramState::Done => 1,
        ProgramState::Idle => 0,
    }
}

/// Ascending order, best first.
#[cfg(test)]
fn record_order(
    left: &ProgramRecord,
    left_pane: u64,
    right: &ProgramRecord,
    right_pane: u64,
) -> std::cmp::Ordering {
    urgency(right)
        .cmp(&urgency(left))
        .then_with(|| right.updated().cmp(&left.updated()))
        .then_with(|| left.id().cmp(right.id()))
        .then_with(|| left_pane.cmp(&right_pane))
}
