//! Daemon-owned OSC 7501 program status records.
//!
//! A pane's PTY reader feeds the released `rio-vt` parser, which validates a
//! report before it reaches this module. Nothing here parses an OSC sequence
//! again or re-implements the protocol's byte/id/text limits:
//! [`ProgramStatusStore::report`] takes an already-validated [`rio_vt::event::ProgramStatus`] and applies the
//! remaining state semantics.
//!
//! The pane daemon owns one [`ProgramStatusStore`] for its whole lifetime. It
//! keeps a root record (`id == ""`) and hierarchical children, resolves `app`
//! from the nearest current ancestor, bounds the table at [`MAX_RECORDS`] with
//! least-recently-updated eviction, and separates update recency from the
//! session-lifetime semantic and result counters. Consumers receive a
//! [`ProgramSnapshot`] on attach and [`ProgramDelta`] values for ordered live
//! changes; a [`ProgramStatusClient`] applies either frame without guessing a
//! version from payload contents.
//!
//! This is transient pane state. It is not written to configuration, the
//! machine tree, or disk scrollback. The store's only serialized form is the
//! existing in-memory daemon handoff record, which must preserve the session
//! identity, both counters, the recency high-water mark, the explicit `app`
//! payload, and the records. Projection frames serialize for the wire but are
//! not persisted.
//!
//! [`plain_program_text`] is the display-side sanitizer for program-origin
//! text. It intentionally does not modify what the store retained.

use std::collections::{BTreeMap, BTreeSet};

use rio_vt::event::{BlockedKind as RioBlockedKind, ProgramState as RioProgramState};
use serde::{Deserialize, Serialize};

/// The released protocol's per-pane record cap. A new record past the cap
/// evicts the record with the oldest update recency.
pub const MAX_RECORDS: usize = 256;

/// A retained program state. `clear` is an operation, never a stored state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

/// Why a `Blocked` program is waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

/// One record as consumers see it.
///
/// `app` is the effective inheritance result, not the report's explicit value.
/// `updated` is the pane's authoritative, cross-pane-comparable recency.
/// `semantic_version` advances only when the effective projection changes;
/// `result_version` advances only when a result is entered or the blocked
/// reason kind changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProgramRecord {
    id: String,
    state: ProgramState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<BlockedKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    progress: Option<u8>,
    updated: u64,
    semantic_version: u64,
    result_version: u64,
}

impl ProgramRecord {
    /// The slash-separated record path; `""` is the root record.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> ProgramState {
        self.state
    }

    /// The blocked reason, present only while `state()` is `Blocked`.
    pub fn kind(&self) -> Option<BlockedKind> {
        self.kind
    }

    /// The current effective app after nearest-ancestor inheritance.
    pub fn app(&self) -> Option<&str> {
        self.app.as_deref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// 0-100, only meaningful on `Working` or `Blocked`; `None` is
    /// indeterminate.
    pub fn progress(&self) -> Option<u8> {
        self.progress
    }

    /// Authoritative update recency (`Unix` nanoseconds, monotonic per pane).
    /// Comparable across panes on one machine.
    pub fn updated(&self) -> u64 {
        self.updated
    }

    pub fn semantic_version(&self) -> u64 {
        self.semantic_version
    }

    pub fn result_version(&self) -> u64 {
        self.result_version
    }
}

/// A complete authoritative projection for a fresh consumer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProgramSnapshot {
    session_id: String,
    semantic_version: u64,
    result_version: u64,
    records: Vec<ProgramRecord>,
}

impl ProgramSnapshot {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The session's semantic counter high-water mark.
    pub fn semantic_version(&self) -> u64 {
        self.semantic_version
    }

    /// The session's result counter high-water mark.
    pub fn result_version(&self) -> u64 {
        self.result_version
    }

    pub fn records(&self) -> &[ProgramRecord] {
        &self.records
    }
}

/// One ordered live change. Only records whose effective projection changed
/// (including inherited-app changes) plus removed ids travel here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProgramDelta {
    session_id: String,
    semantic_version: u64,
    result_version: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed: Vec<ProgramRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed: Vec<String>,
}

impl ProgramDelta {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The semantic counter after this change.
    pub fn semantic_version(&self) -> u64 {
        self.semantic_version
    }

    /// The result counter after this change.
    pub fn result_version(&self) -> u64 {
        self.result_version
    }

    pub fn changed(&self) -> &[ProgramRecord] {
        &self.changed
    }

    pub fn removed(&self) -> &[String] {
        &self.removed
    }
}

/// A baseline for a fresh/reconnecting consumer or an ordered live update.
/// The distinction is explicit so a baseline is never mistaken for a live
/// transition (in particular for notification/unread consumers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ProgramStatusFrame {
    Baseline(ProgramSnapshot),
    Update(ProgramDelta),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredRecord {
    state: ProgramState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<BlockedKind>,
    /// The report's explicit app, kept for inheritance. `None` means "inherit
    /// from the nearest current ancestor"; it is never exposed as the record's
    /// effective `app()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    progress: Option<u8>,
    updated: u64,
    semantic_version: u64,
    result_version: u64,
}

/// The canonical payload of one accepted report. Keeping it in one value
/// avoids duplicating the insert/update argument list.
struct ReportedFields {
    state: ProgramState,
    kind: Option<BlockedKind>,
    app: Option<String>,
    title: Option<String>,
    message: Option<String>,
    progress: Option<u8>,
}

/// One surviving record whose effective app may change when an app-bearing
/// ancestor is added, replaced, or removed.
struct InheritedCandidate {
    id: String,
    app: Option<String>,
}

/// The pane daemon's program-status authority.
///
/// Clone/Debug/Serialize/Deserialize exist for the existing transient Unix
/// handoff record. Clone is not used on the report path.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProgramStatusStore {
    session_id: String,
    #[serde(default)]
    semantic_version: u64,
    #[serde(default)]
    result_version: u64,
    /// The recency high-water mark. It is serialized so a same-session handoff
    /// cannot move recency backwards or reuse an older timestamp.
    #[serde(default)]
    next_updated: u64,
    #[serde(default)]
    records: BTreeMap<String, StoredRecord>,
}

impl Default for ProgramStatusStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgramStatusStore {
    /// A fresh pane lifetime: new identity and all counters at zero.
    pub fn new() -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            semantic_version: 0,
            result_version: 0,
            next_updated: 0,
            records: BTreeMap::new(),
        }
    }

    /// The session-lifetime identity. It survives a supported handoff and is
    /// replaced only by [`Self::new`], never by daemon process identity.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The complete current projection. Used for attach/reconnect baselines.
    pub fn snapshot(&self) -> ProgramSnapshot {
        let records = self
            .records
            .iter()
            .map(|(id, record)| record_from_store(id, record, effective_app(&self.records, id)))
            .collect();
        ProgramSnapshot {
            session_id: self.session_id.clone(),
            semantic_version: self.semantic_version,
            result_version: self.result_version,
            records,
        }
    }

    /// Apply one already-validated Rio report.
    ///
    /// Whole replacement applies to the addressed record: fields absent from
    /// the report remove their previous values. A `clear` report removes the
    /// addressed record and its descendants, or every record when it has no
    /// `id`. A report with an empty explicit id is rejected without touching
    /// state: a malformed id must never alias the root record.
    pub fn report(&mut self, report: rio_vt::event::ProgramStatus) -> Option<ProgramDelta> {
        let rio_vt::event::ProgramStatus {
            state,
            kind,
            app,
            msg,
            title,
            progress,
            id,
        } = report;
        let id = match id {
            None => String::new(),
            // The released parser rejects this before it can reach the daemon.
            // Keeping the guard here is a precondition check, not a second
            // id grammar: an explicit empty path must not become the root.
            Some(id) if id.is_empty() => return None,
            Some(id) => id,
        };
        let Some(state) = retained_state(state) else {
            return self.clear(id);
        };
        let kind = kind
            .filter(|_| state == ProgramState::Blocked)
            .map(blocked_kind);
        // Rio has already filtered progress to Working/Blocked and 0..=100.
        // Normalize a caller-constructed report the same way instead of
        // inventing a second parser or storing a value the protocol forbids.
        let progress = progress
            .filter(|value| *value <= 100)
            .filter(|_| matches!(state, ProgramState::Working | ProgramState::Blocked));
        let fields = ReportedFields {
            state,
            kind,
            app,
            title,
            message: msg,
            progress,
        };
        if self.records.contains_key(&id) {
            self.update_report(id, fields)
        } else {
            self.insert_report(id, fields)
        }
    }

    /// A real new prompt: drop idle, working, and blocked records; retain
    /// done/error. Surviving children re-resolve app inheritance.
    pub fn prompt(&mut self) -> Option<ProgramDelta> {
        self.drop_transient()
    }

    /// The attached process exited: same lifecycle cleanup as a new prompt.
    pub fn exited(&mut self) -> Option<ProgramDelta> {
        self.drop_transient()
    }

    /// RIS: remove every record without resetting the session or either
    /// counter. Recovery after clear/eviction/RIS therefore cannot reuse an
    /// older result version.
    pub fn reset(&mut self) -> Option<ProgramDelta> {
        if self.records.is_empty() {
            return None;
        }
        let removed: Vec<String> = self.records.keys().cloned().collect();
        self.records.clear();
        let semantic = self.bump_semantic();
        self.build_delta(removed, BTreeSet::new(), semantic, self.result_version)
    }

    fn clear(&mut self, root: String) -> Option<ProgramDelta> {
        let prefix = if root.is_empty() {
            String::new()
        } else {
            format!("{root}/")
        };
        let removed: Vec<String> = self
            .records
            .keys()
            .filter(|id| {
                root.is_empty()
                    || id.as_str() == root.as_str()
                    || id.as_str().starts_with(prefix.as_str())
            })
            .cloned()
            .collect();
        if removed.is_empty() {
            return None;
        }
        for id in &removed {
            let _ = self.records.remove(id);
        }
        let semantic = self.bump_semantic();
        // Every record that could inherit from the removed subtree was itself
        // removed with it.
        self.build_delta(removed, BTreeSet::new(), semantic, self.result_version)
    }

    fn drop_transient(&mut self) -> Option<ProgramDelta> {
        if self.records.is_empty() {
            return None;
        }
        let victims: Vec<String> = self
            .records
            .iter()
            .filter(|(_, record)| {
                matches!(
                    record.state,
                    ProgramState::Idle | ProgramState::Working | ProgramState::Blocked
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.remove_records(victims)
    }

    fn insert_report(&mut self, id: String, fields: ReportedFields) -> Option<ProgramDelta> {
        let ReportedFields {
            state,
            kind,
            app,
            title,
            message,
            progress,
        } = fields;
        let to_evict = (self.records.len() + 1).saturating_sub(MAX_RECORDS);
        let mut victims = BTreeSet::new();
        for _ in 0..to_evict {
            let Some(victim) = self.lru_victim(&victims) else {
                break;
            };
            victims.insert(victim);
        }
        // A new record can change every descendant's effective app. Evicting
        // an app-bearing ancestor can do the same to its surviving descendants.
        // Capture only the records that could inherit (their own `app` absent)
        // and compare after the mutation; ordinary reports clone nothing.
        let candidates = if app.is_some() || !victims.is_empty() {
            self.snapshot_inherited(&victims)
        } else {
            Vec::new()
        };
        let mut removed = Vec::with_capacity(victims.len());
        for victim in &victims {
            if self.records.remove(victim).is_some() {
                removed.push(victim.clone());
            }
        }
        let semantic = self.bump_semantic();
        let result = if is_result_state(state) {
            self.bump_result()
        } else {
            0
        };
        let updated = self.next_updated();
        let _ = self.records.insert(
            id.clone(),
            StoredRecord {
                state,
                kind,
                app,
                title,
                message,
                progress,
                updated,
                semantic_version: semantic,
                result_version: result,
            },
        );
        let (tick, mut changed) = self.publish_inherited(candidates, Some(semantic));
        changed.insert(id);
        self.build_delta(
            removed,
            changed,
            tick.unwrap_or(semantic),
            self.result_version,
        )
    }

    fn update_report(&mut self, id: String, fields: ReportedFields) -> Option<ProgramDelta> {
        let ReportedFields {
            state,
            kind,
            app,
            title,
            message,
            progress,
        } = fields;
        let (old_state, old_kind, old_result) = {
            let record = self.records.get(&id)?;
            (record.state, record.kind, record.result_version)
        };
        let old_effective = effective_app(&self.records, &id).map(str::to_owned);
        let parent_effective = parent_id(&id)
            .and_then(|parent| effective_app(&self.records, parent))
            .map(str::to_owned);
        let new_effective = app
            .as_deref()
            .or(parent_effective.as_deref())
            .map(str::to_owned);
        let projected_changed = {
            let record = self.records.get(&id)?;
            record.state != state
                || record.kind != kind
                || record.title != title
                || record.message != message
                || record.progress != progress
                || old_effective.as_deref() != new_effective.as_deref()
        };

        if !projected_changed {
            // Every accepted refresh carries authoritative recency. Even the
            // same local representative may now outrank another pane; keeping
            // non-representatives current also prevents stale ties after clear.
            // Consumers gate rendering on the effective displayed summary.
            let updated = self.next_updated();
            {
                let record = self
                    .records
                    .get_mut(&id)
                    .expect("the addressed record was checked before mutation");
                record.state = state;
                record.kind = kind;
                record.app = app;
                record.title = title;
                record.message = message;
                record.progress = progress;
                record.updated = updated;
            }
            let mut changed = BTreeSet::new();
            changed.insert(id);
            return self.build_delta(
                Vec::new(),
                changed,
                self.semantic_version,
                self.result_version,
            );
        }

        let result_advanced = result_transition(old_state, old_kind, state, kind);
        let mut excluded = BTreeSet::new();
        excluded.insert(id.clone());
        let candidates = if old_effective.as_deref() != new_effective.as_deref() {
            self.snapshot_inherited(&excluded)
        } else {
            Vec::new()
        };
        let semantic = self.bump_semantic();
        // Leaving a result state keeps the last result identity. Re-entering
        // done/error/blocked then bumps the counter, so an identical done
        // payload after working is newer than the result a consumer already
        // acknowledged; clear/evict/recreate cannot alias it either.
        let result = if result_advanced {
            self.bump_result()
        } else {
            old_result
        };
        let updated = self.next_updated();
        {
            let record = self
                .records
                .get_mut(&id)
                .expect("the addressed record was checked before mutation");
            record.state = state;
            record.kind = kind;
            record.app = app;
            record.title = title;
            record.message = message;
            record.progress = progress;
            record.updated = updated;
            record.semantic_version = semantic;
            record.result_version = result;
        }
        let (tick, mut changed) = self.publish_inherited(candidates, Some(semantic));
        changed.insert(id);
        self.build_delta(
            Vec::new(),
            changed,
            tick.unwrap_or(semantic),
            self.result_version,
        )
    }

    fn remove_records(&mut self, victims: Vec<String>) -> Option<ProgramDelta> {
        if victims.is_empty() {
            return None;
        }
        let victims: BTreeSet<String> = victims.into_iter().collect();
        // Only removing a record with an explicit app can change a surviving
        // descendant's effective app; records with no explicit app contribute
        // no value to inherit.
        let app_removed = victims.iter().any(|id| {
            self.records
                .get(id)
                .is_some_and(|record| record.app.is_some())
        });
        let candidates = if app_removed {
            self.snapshot_inherited(&victims)
        } else {
            Vec::new()
        };
        let mut removed = Vec::with_capacity(victims.len());
        for id in &victims {
            if self.records.remove(id).is_some() {
                removed.push(id.clone());
            }
        }
        let semantic = self.bump_semantic();
        let (tick, changed) = self.publish_inherited(candidates, Some(semantic));
        self.build_delta(
            removed,
            changed,
            tick.unwrap_or(semantic),
            self.result_version,
        )
    }

    fn snapshot_inherited(&self, excluded: &BTreeSet<String>) -> Vec<InheritedCandidate> {
        self.records
            .iter()
            .filter(|(id, record)| record.app.is_none() && !excluded.contains(id.as_str()))
            .map(|(id, _)| InheritedCandidate {
                id: id.clone(),
                app: effective_app(&self.records, id).map(str::to_owned),
            })
            .collect()
    }

    /// Re-resolve each candidate's effective app after the mutation and give
    /// every changed survivor the same semantic tick as the operation.
    fn publish_inherited(
        &mut self,
        candidates: Vec<InheritedCandidate>,
        tick: Option<u64>,
    ) -> (Option<u64>, BTreeSet<String>) {
        let changed_ids: Vec<String> = candidates
            .into_iter()
            .filter_map(|candidate| {
                // A removed/evicted candidate cannot receive a record change;
                // exclude it before deciding whether the operation needs a new
                // semantic tick.
                if !self.records.contains_key(&candidate.id) {
                    return None;
                }
                let new = effective_app(&self.records, &candidate.id);
                (new != candidate.app.as_deref()).then_some(candidate.id)
            })
            .collect();
        if changed_ids.is_empty() {
            return (tick, BTreeSet::new());
        }
        let tick = tick.unwrap_or_else(|| self.bump_semantic());
        let mut changed = BTreeSet::new();
        for id in changed_ids {
            if let Some(record) = self.records.get_mut(&id) {
                record.semantic_version = tick;
                changed.insert(id);
            }
        }
        (Some(tick), changed)
    }

    fn lru_victim(&self, excluded: &BTreeSet<String>) -> Option<String> {
        self.records
            .iter()
            .filter(|(id, _)| !excluded.contains(id.as_str()))
            .min_by(|(left_id, left), (right_id, right)| {
                left.updated
                    .cmp(&right.updated)
                    // A timestamp tie is only possible in restored data; keep
                    // eviction deterministic by preferring to drop the greater
                    // path.
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(id, _)| id.clone())
    }

    fn build_delta(
        &self,
        mut removed: Vec<String>,
        changed: BTreeSet<String>,
        semantic_version: u64,
        result_version: u64,
    ) -> Option<ProgramDelta> {
        if removed.is_empty() && changed.is_empty() {
            return None;
        }
        removed.sort();
        let changed = changed
            .into_iter()
            .filter_map(|id| {
                self.records
                    .get(&id)
                    .map(|record| record_from_store(&id, record, effective_app(&self.records, &id)))
            })
            .collect();
        Some(ProgramDelta {
            session_id: self.session_id.clone(),
            semantic_version,
            result_version,
            changed,
            removed,
        })
    }

    fn next_updated(&mut self) -> u64 {
        let now = unix_nanos();
        let updated = now.max(self.next_updated.saturating_add(1));
        self.next_updated = updated;
        updated
    }

    fn bump_semantic(&mut self) -> u64 {
        self.semantic_version = self.semantic_version.saturating_add(1);
        self.semantic_version
    }

    fn bump_result(&mut self) -> u64 {
        self.result_version = self.result_version.saturating_add(1);
        self.result_version
    }
}

/// The consuming side of the ordered projection.
///
/// `apply` validates a frame before touching state, so a rejected frame cannot
/// leave a half-applied record set. A complete baseline replaces the
/// projection atomically and a different session identity discards the old
/// projection. A same-session frame whose counters go backwards is rejected.
/// A live update for a different session is ignored: without that session's
/// baseline it cannot reconstruct a coherent record set.
#[derive(Clone, Debug, Default)]
pub struct ProgramStatusClient {
    session_id: Option<String>,
    semantic_version: u64,
    result_version: u64,
    records: BTreeMap<String, ProgramRecord>,
}

impl ProgramStatusClient {
    /// Returns whether the effective record projection changed. Counter-only
    /// advances are adopted but do not report a visible change.
    pub fn apply(&mut self, frame: &ProgramStatusFrame) -> bool {
        match frame {
            ProgramStatusFrame::Baseline(snapshot) => self.apply_baseline(snapshot),
            ProgramStatusFrame::Update(delta) => self.apply_delta(delta),
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn records(&self) -> impl Iterator<Item = &ProgramRecord> {
        self.records.values()
    }

    /// A complete baseline of the client's current effective projection.
    ///
    /// The records carry the versions this client has actually applied, so the
    /// result is a legal same-session `Baseline` for replacing a pending queue.
    /// Before the first baseline has been applied there is no session identity
    /// to carry; the returned snapshot then has an empty `session_id` and no
    /// records, which [`Self::apply`] rejects as a baseline. Callers must only
    /// send this value after [`Self::session_id`] is `Some`.
    pub fn snapshot(&self) -> ProgramSnapshot {
        ProgramSnapshot {
            session_id: self.session_id.clone().unwrap_or_default(),
            semantic_version: self.semantic_version,
            result_version: self.result_version,
            records: self.records.values().cloned().collect(),
        }
    }

    /// Priority blocked, error, working, done, idle; within one state the most
    /// recently updated record, then the lexicographically smaller path.
    pub fn representative(&self) -> Option<&ProgramRecord> {
        self.records.values().min_by(|left, right| {
            state_rank(left.state)
                .cmp(&state_rank(right.state))
                .then_with(|| right.updated.cmp(&left.updated))
                .then_with(|| left.id.cmp(&right.id))
        })
    }

    pub fn semantic_version(&self) -> u64 {
        self.semantic_version
    }

    pub fn result_version(&self) -> u64 {
        self.result_version
    }

    fn apply_baseline(&mut self, snapshot: &ProgramSnapshot) -> bool {
        if snapshot.session_id.is_empty() || snapshot.records.len() > MAX_RECORDS {
            return false;
        }
        if snapshot.records.iter().any(|record| {
            record.semantic_version > snapshot.semantic_version
                || record.result_version > snapshot.result_version
        }) {
            return false;
        }
        let same_session = self.session_id.as_deref() == Some(snapshot.session_id.as_str());
        if same_session
            && (snapshot.semantic_version < self.semantic_version
                || snapshot.result_version < self.result_version)
        {
            return false;
        }
        let mut records = BTreeMap::new();
        for record in &snapshot.records {
            if records.insert(record.id.clone(), record.clone()).is_some() {
                return false;
            }
        }
        let records_changed = records != self.records;
        self.session_id = Some(snapshot.session_id.clone());
        self.semantic_version = snapshot.semantic_version;
        self.result_version = snapshot.result_version;
        self.records = records;
        !same_session || records_changed
    }

    fn apply_delta(&mut self, delta: &ProgramDelta) -> bool {
        let Some(session_id) = self.session_id.as_deref() else {
            return false;
        };
        if delta.session_id.as_str() != session_id {
            return false;
        }
        if delta.semantic_version < self.semantic_version
            || delta.result_version < self.result_version
        {
            return false;
        }
        if delta.changed.len() > MAX_RECORDS
            || delta.removed.len() > MAX_RECORDS
            || delta.changed.len() + delta.removed.len() > MAX_RECORDS + 1
        {
            return false;
        }
        let mut changed_ids = BTreeSet::new();
        for record in &delta.changed {
            if !changed_ids.insert(record.id.as_str()) {
                return false;
            }
            if record.semantic_version > delta.semantic_version
                || record.result_version > delta.result_version
            {
                return false;
            }
        }
        let mut removed_ids = BTreeSet::new();
        for id in &delta.removed {
            if !removed_ids.insert(id.as_str()) {
                return false;
            }
        }
        if changed_ids.iter().any(|id| removed_ids.contains(*id)) {
            return false;
        }
        let mut resulting = self.records.len();
        for id in &removed_ids {
            if self.records.contains_key(*id) {
                resulting -= 1;
            }
        }
        for record in &delta.changed {
            if !self.records.contains_key(record.id.as_str()) {
                resulting += 1;
            }
        }
        if resulting > MAX_RECORDS {
            return false;
        }
        let mut projection_changed = false;
        for id in &delta.removed {
            if self.records.remove(id).is_some() {
                projection_changed = true;
            }
        }
        for record in &delta.changed {
            if let Some(previous) = self.records.insert(record.id.clone(), record.clone()) {
                if &previous != record {
                    projection_changed = true;
                }
            } else {
                projection_changed = true;
            }
        }
        self.semantic_version = delta.semantic_version;
        self.result_version = delta.result_version;
        projection_changed
    }
}

fn retained_state(state: RioProgramState) -> Option<ProgramState> {
    match state {
        RioProgramState::Idle => Some(ProgramState::Idle),
        RioProgramState::Working => Some(ProgramState::Working),
        RioProgramState::Done => Some(ProgramState::Done),
        RioProgramState::Blocked => Some(ProgramState::Blocked),
        RioProgramState::Error => Some(ProgramState::Error),
        RioProgramState::Clear => None,
    }
}

fn blocked_kind(kind: RioBlockedKind) -> BlockedKind {
    match kind {
        RioBlockedKind::Permission => BlockedKind::Permission,
        RioBlockedKind::Question => BlockedKind::Question,
        RioBlockedKind::Auth => BlockedKind::Auth,
    }
}

fn is_result_state(state: ProgramState) -> bool {
    matches!(
        state,
        ProgramState::Done | ProgramState::Error | ProgramState::Blocked
    )
}

fn result_transition(
    old_state: ProgramState,
    old_kind: Option<BlockedKind>,
    new_state: ProgramState,
    new_kind: Option<BlockedKind>,
) -> bool {
    (old_state != new_state && is_result_state(new_state))
        || (old_state == ProgramState::Blocked
            && new_state == ProgramState::Blocked
            && old_kind != new_kind)
}

fn parent_id(id: &str) -> Option<&str> {
    if id.is_empty() {
        None
    } else {
        Some(id.rsplit_once('/').map_or("", |(parent, _)| parent))
    }
}

/// Resolve the effective app from the nearest current ancestor, including the
/// record itself. Missing intermediate records are skipped, so eviction of a
/// parent falls through to the next retained app.
fn effective_app<'a>(records: &'a BTreeMap<String, StoredRecord>, id: &str) -> Option<&'a str> {
    let mut current = Some(id);
    while let Some(key) = current {
        if let Some(record) = records.get(key)
            && let Some(app) = record.app.as_deref()
        {
            return Some(app);
        }
        current = if key.is_empty() {
            None
        } else {
            Some(key.rsplit_once('/').map_or("", |(parent, _)| parent))
        };
    }
    None
}

fn record_from_store(id: &str, record: &StoredRecord, app: Option<&str>) -> ProgramRecord {
    ProgramRecord {
        id: id.to_owned(),
        state: record.state,
        kind: record.kind,
        app: app.map(str::to_owned),
        title: record.title.clone(),
        message: record.message.clone(),
        progress: record.progress,
        updated: record.updated,
        semantic_version: record.semantic_version,
        result_version: record.result_version,
    }
}

fn state_rank(state: ProgramState) -> u8 {
    match state {
        ProgramState::Blocked => 0,
        ProgramState::Error => 1,
        ProgramState::Working => 2,
        ProgramState::Done => 3,
        ProgramState::Idle => 4,
    }
}

fn unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Render program-origin text as plain, non-executable display text.
///
/// The stored payload is already validated by Rio (no control characters),
/// and this function never changes it. For display outside the grid it
/// neutralizes bidirectional controls with a visible replacement and removes
/// invisible non-directional formatting that can hide or reorder text.
/// Zero-width joiners are preserved: they are part of legitimate emoji and
/// script sequences, not a direction override.
pub fn plain_program_text(text: &str) -> String {
    if !text.chars().any(is_unsafe_program_format) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if is_direction_format(character) || character.is_control() {
            // Direction controls and any control that reached the display
            // boundary are visibly neutralized rather than allowed to reorder
            // or restructure the line.
            out.push('\u{FFFD}');
        } else if !is_invisible_format(character) {
            // Zero-width joiner U+200D is intentionally neither direction nor
            // invisible here, so legitimate emoji/script sequences survive.
            out.push(character);
        }
    }
    out
}

fn is_direction_format(character: char) -> bool {
    matches!(
        character,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2061}'..='\u{206F}'
    )
}

fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200C}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{2060}'
            | '\u{E0000}'..='\u{E007F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

fn is_unsafe_program_format(character: char) -> bool {
    is_direction_format(character) || is_invisible_format(character) || character.is_control()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rio_vt::event::{
        BlockedKind as RioBlockedKind, ProgramState as RioProgramState,
        ProgramStatus as RioProgramStatus,
    };

    fn report(
        store: &mut ProgramStatusStore,
        id: Option<&str>,
        state: RioProgramState,
        kind: Option<RioBlockedKind>,
        app: Option<&str>,
        title: Option<&str>,
        message: Option<&str>,
    ) -> Option<ProgramDelta> {
        store.report(RioProgramStatus {
            state,
            kind,
            app: app.map(str::to_owned),
            msg: message.map(str::to_owned),
            title: title.map(str::to_owned),
            progress: None,
            id: id.map(str::to_owned),
        })
    }

    fn record<'a>(records: &'a [ProgramRecord], id: &str) -> &'a ProgramRecord {
        records
            .iter()
            .find(|record| record.id() == id)
            .expect("record is present")
    }

    fn record_in_store<'a>(snapshot: &'a ProgramSnapshot, id: &str) -> &'a ProgramRecord {
        record(snapshot.records(), id)
    }

    #[test]
    fn root_and_children_inherit_the_nearest_current_app() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            None,
            RioProgramState::Idle,
            None,
            Some("shell"),
            None,
            None,
        );
        report(
            &mut store,
            Some("worker"),
            RioProgramState::Working,
            None,
            None,
            Some("build"),
            Some("building"),
        );
        let snapshot = store.snapshot();
        assert_eq!(record_in_store(&snapshot, "worker").app(), Some("shell"));

        // Whole replacement: the absent title removes the old value, and the
        // absent app re-inherits from the root.
        let delta = report(
            &mut store,
            Some("worker"),
            RioProgramState::Working,
            None,
            None,
            None,
            Some("still building"),
        )
        .expect("a changed record is published");
        assert!(delta.changed()[0].title().is_none());

        // Replacing the parent app must publish the child's effective change.
        let delta = report(
            &mut store,
            None,
            RioProgramState::Idle,
            None,
            Some("editor"),
            None,
            None,
        )
        .expect("the root change is published");
        assert_eq!(
            record(delta.changed(), "worker").app(),
            Some("editor"),
            "a parent app replacement updates the child's effective app"
        );
        assert_eq!(
            record_in_store(&store.snapshot(), "worker").app(),
            Some("editor")
        );

        assert!(
            report(
                &mut store,
                Some(""),
                RioProgramState::Working,
                None,
                None,
                None,
                None,
            )
            .is_none(),
            "an explicit empty id must not alias the root record"
        );
    }

    #[test]
    fn clear_removes_the_addressed_subtree_or_every_record() {
        let mut store = ProgramStatusStore::new();
        for (id, state) in [
            (None, RioProgramState::Working),
            (Some("a"), RioProgramState::Working),
            (Some("a/b"), RioProgramState::Working),
            (Some("b"), RioProgramState::Done),
        ] {
            report(&mut store, id, state, None, None, None, None);
        }
        let delta = report(
            &mut store,
            Some("a"),
            RioProgramState::Clear,
            None,
            None,
            None,
            None,
        )
        .expect("the subtree removal is published");
        assert_eq!(
            delta.removed(),
            ["a".to_owned(), "a/b".to_owned()].as_slice()
        );
        let snapshot = store.snapshot();
        assert!(
            snapshot
                .records()
                .iter()
                .all(|record| record.id() != "a" && record.id() != "a/b")
        );
        assert_eq!(
            record_in_store(&snapshot, "").state(),
            ProgramState::Working
        );
        assert_eq!(record_in_store(&snapshot, "b").state(), ProgramState::Done);

        let delta = report(
            &mut store,
            None,
            RioProgramState::Clear,
            None,
            None,
            None,
            None,
        )
        .expect("the root clear is published");
        assert_eq!(delta.removed(), ["".to_owned(), "b".to_owned()].as_slice());
        assert!(store.snapshot().records().is_empty());
    }

    #[test]
    fn prompt_keeps_results_and_republishes_surviving_inheritance() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            None,
            RioProgramState::Working,
            None,
            Some("shell"),
            None,
            None,
        );
        report(
            &mut store,
            Some("task"),
            RioProgramState::Done,
            None,
            None,
            None,
            Some("finished"),
        );
        report(
            &mut store,
            Some("other"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        );
        report(
            &mut store,
            Some("idle"),
            RioProgramState::Idle,
            None,
            None,
            None,
            None,
        );
        let delta = store.prompt().expect("transient records are removed");
        assert_eq!(
            delta.removed(),
            ["".to_owned(), "idle".to_owned(), "other".to_owned()].as_slice()
        );
        assert_eq!(record(delta.changed(), "task").app(), None);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(record_in_store(&snapshot, "task").app(), None);
        assert_eq!(
            record_in_store(&snapshot, "task").state(),
            ProgramState::Done
        );
        assert!(store.prompt().is_none(), "nothing transient is left");
    }

    #[test]
    fn a_full_table_evicts_the_least_recently_updated_record() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            None,
            RioProgramState::Idle,
            None,
            Some("shell"),
            None,
            None,
        );
        for index in 0..MAX_RECORDS {
            let id = format!("task-{index}");
            report(
                &mut store,
                Some(&id),
                RioProgramState::Working,
                None,
                None,
                None,
                None,
            );
        }
        let snapshot = store.snapshot();
        assert_eq!(snapshot.records().len(), MAX_RECORDS);
        assert!(
            snapshot.records().iter().all(|record| record.id() != ""),
            "the oldest root record was evicted"
        );
        assert_eq!(
            record_in_store(&snapshot, "task-0").app(),
            None,
            "evicting the app-bearing ancestor republishes the survivors"
        );
    }

    #[test]
    fn handoff_serde_preserves_identity_explicit_apps_counters_and_recency() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            None,
            RioProgramState::Idle,
            None,
            Some("shell"),
            None,
            None,
        );
        report(
            &mut store,
            Some("worker"),
            RioProgramState::Working,
            None,
            Some("explicit"),
            None,
            None,
        );
        report(
            &mut store,
            Some("child"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        );
        let snapshot = store.snapshot();
        let encoded = serde_json::to_vec(&store).expect("the handoff form serializes");
        let mut restored: ProgramStatusStore =
            serde_json::from_slice(&encoded).expect("the handoff form deserializes");
        assert_eq!(restored.session_id(), store.session_id());
        assert_eq!(restored.snapshot(), snapshot);

        // The explicit app was carried: replacing the root app changes the
        // inheriting child but must not touch the explicit worker.
        let delta = report(
            &mut restored,
            None,
            RioProgramState::Idle,
            None,
            Some("editor"),
            None,
            None,
        )
        .expect("the root app change is published");
        assert_eq!(record(delta.changed(), "child").app(), Some("editor"));
        assert!(delta.changed().iter().all(|record| record.id() != "worker"));
        assert_eq!(
            record_in_store(&restored.snapshot(), "worker").app(),
            Some("explicit")
        );

        // The recency high-water was carried: a fresh report is strictly newer
        // than every restored record.
        let previous_max = restored
            .snapshot()
            .records()
            .iter()
            .map(ProgramRecord::updated)
            .max()
            .expect("records were restored");
        let delta = report(
            &mut restored,
            Some("fresh"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        )
        .expect("the fresh record is published");
        assert!(record(delta.changed(), "fresh").updated() > previous_max);
    }

    #[test]
    fn identical_reports_preserve_versions_and_publish_authoritative_recency() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        );
        report(
            &mut store,
            Some("b"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        );
        let before = store.snapshot();
        assert_eq!(before.semantic_version(), 2);
        assert_eq!(before.result_version(), 2);

        let delta = report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        )
        .expect("refreshing the non-representative record changes the tie");
        assert_eq!(delta.semantic_version(), before.semantic_version());
        assert_eq!(delta.result_version(), before.result_version());
        assert_eq!(delta.changed().len(), 1);
        assert_eq!(delta.changed()[0].id(), "a");
        assert_eq!(
            delta.changed()[0].semantic_version(),
            record_in_store(&before, "a").semantic_version()
        );
        assert_eq!(
            delta.changed()[0].result_version(),
            record_in_store(&before, "a").result_version()
        );
        assert!(delta.changed()[0].updated() > record_in_store(&before, "a").updated());

        let refreshed = report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        )
        .expect("the same representative's recency is needed by cross-pane consumers");
        assert_eq!(refreshed.semantic_version(), before.semantic_version());
        assert_eq!(refreshed.result_version(), before.result_version());
        assert!(refreshed.changed()[0].updated() > delta.changed()[0].updated());

        let mut client = ProgramStatusClient::default();
        assert!(client.apply(&ProgramStatusFrame::Baseline(before)));
        assert!(client.apply(&ProgramStatusFrame::Update(delta)));
        assert_eq!(
            client.representative().map(ProgramRecord::id),
            Some("a"),
            "a recency-only frame moves the consumer's representative"
        );
    }

    #[test]
    fn result_versions_follow_result_transitions_and_never_reuse() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            Some("a"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        );
        let working = store.snapshot();
        assert_eq!(record_in_store(&working, "a").result_version(), 0);

        let done = report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        )
        .expect("entering done is a result");
        let first_result = record(done.changed(), "a").result_version();
        assert_eq!(first_result, 1);

        report(
            &mut store,
            Some("a"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        );
        let done_again = report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        )
        .expect("re-entering done is a new result");
        let second_result = record(done_again.changed(), "a").result_version();
        assert!(second_result > first_result);

        let blocked = report(
            &mut store,
            Some("a"),
            RioProgramState::Blocked,
            Some(RioBlockedKind::Permission),
            None,
            None,
            None,
        )
        .expect("entering blocked is a result");
        let blocked_result = record(blocked.changed(), "a").result_version();
        assert!(blocked_result > second_result);

        let routine = report(
            &mut store,
            Some("a"),
            RioProgramState::Blocked,
            Some(RioBlockedKind::Permission),
            None,
            Some("new title"),
            Some("routine refresh"),
        )
        .expect("the text change is still a semantic change");
        assert_eq!(
            record(routine.changed(), "a").result_version(),
            blocked_result,
            "same-state text does not create a new result"
        );

        let reason_changed = report(
            &mut store,
            Some("a"),
            RioProgramState::Blocked,
            Some(RioBlockedKind::Question),
            None,
            Some("new title"),
            Some("routine refresh"),
        )
        .expect("a blocked-kind change is a new result");
        let question_result = record(reason_changed.changed(), "a").result_version();
        assert!(question_result > blocked_result);

        store.reset().expect("records are removed");
        let recreated = report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        )
        .expect("recreating the same id enters done again");
        assert!(
            record(recreated.changed(), "a").result_version() > question_result,
            "clear/recreate cannot alias an older result"
        );
        assert!(
            store.snapshot().semantic_version()
                >= record(recreated.changed(), "a").semantic_version()
        );
    }

    #[test]
    fn client_applies_frames_atomically_and_rejects_rollback_and_foreign_sessions() {
        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            Some("a"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        );
        let old_baseline = store.snapshot();
        let update = report(
            &mut store,
            Some("a"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        )
        .expect("the update is published");
        let new_baseline = store.snapshot();

        let mut client = ProgramStatusClient::default();
        assert!(client.apply(&ProgramStatusFrame::Baseline(old_baseline.clone())));
        assert!(client.apply(&ProgramStatusFrame::Update(update.clone())));
        assert_eq!(client.records().count(), 1);
        assert_eq!(
            client.records().next().map(ProgramRecord::state),
            Some(ProgramState::Done)
        );
        let applied = client.snapshot();
        assert_eq!(
            applied.session_id(),
            client
                .session_id()
                .expect("the applied baseline established a session")
        );
        assert_eq!(applied.semantic_version(), client.semantic_version());
        assert_eq!(applied.result_version(), client.result_version());
        assert_eq!(applied.records().len(), 1);
        let mut replacement = ProgramStatusClient::default();
        assert!(replacement.apply(&ProgramStatusFrame::Baseline(applied)));
        assert_eq!(replacement.records().count(), 1);

        assert!(
            !client.apply(&ProgramStatusFrame::Baseline(new_baseline)),
            "a baseline equal to the applied projection is not a visible change"
        );
        assert!(
            !client.apply(&ProgramStatusFrame::Baseline(old_baseline)),
            "an older same-session baseline is rejected"
        );
        assert_eq!(client.records().count(), 1);
        assert_eq!(
            client.records().next().map(ProgramRecord::state),
            Some(ProgramState::Done)
        );
        assert!(!client.apply(&ProgramStatusFrame::Update(update)));

        let removal = report(
            &mut store,
            Some("a"),
            RioProgramState::Clear,
            None,
            None,
            None,
            None,
        )
        .expect("the record removal is published");
        assert!(client.apply(&ProgramStatusFrame::Update(removal)));
        assert_eq!(client.records().count(), 0);
        assert!(store.reset().is_none());
    }

    #[test]
    fn client_ignores_an_update_that_has_no_baseline_for_its_session() {
        let mut other = ProgramStatusStore::new();
        report(
            &mut other,
            Some("other"),
            RioProgramState::Done,
            None,
            None,
            None,
            None,
        );
        let foreign = other.snapshot();
        let foreign_delta = other.reset().expect("the record is removed");

        let mut store = ProgramStatusStore::new();
        report(
            &mut store,
            Some("mine"),
            RioProgramState::Working,
            None,
            None,
            None,
            None,
        );
        let mine = store.snapshot();
        let mut client = ProgramStatusClient::default();
        assert!(client.apply(&ProgramStatusFrame::Baseline(mine.clone())));
        assert!(
            !client.apply(&ProgramStatusFrame::Update(foreign_delta)),
            "a delta without its session baseline cannot reconstruct state"
        );
        assert_eq!(
            client.session_id(),
            Some(mine.session_id()),
            "the rejected frame leaves the session untouched"
        );
        assert!(
            client.apply(&ProgramStatusFrame::Baseline(foreign)),
            "a complete baseline for a new session replaces the old one"
        );
        assert_eq!(client.session_id(), Some(other.session_id()));
        assert_eq!(client.records().count(), 1);
        let empty = other.snapshot();
        assert!(client.apply(&ProgramStatusFrame::Baseline(empty)));
        assert_eq!(client.records().count(), 0);
    }

    #[test]
    fn plain_program_text_neutralizes_direction_and_keeps_emoji_joiners() {
        let emoji = "family 👨‍👩‍👧";
        let malicious = format!("deploy {emoji}\u{202E}evil\u{2067}isolated\u{200B}text");
        let clean = plain_program_text(&malicious);
        assert!(
            clean.contains(emoji),
            "legitimate zero-width joiners survive"
        );
        assert!(!clean.contains('\u{202E}'));
        assert!(!clean.contains('\u{2067}'));
        assert!(!clean.contains('\u{200B}'));
        assert!(clean.contains('\u{FFFD}'));
        assert!(!plain_program_text("cargo: building 40%").contains('\u{FFFD}'));
    }
}
