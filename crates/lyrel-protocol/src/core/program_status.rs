//! OSC 7501 program status wire values and the consumer-side projection.
//!
//! This module is the `rio-vt`-free half of the daemon's program-status
//! support. The pane daemon's state owner (`ProgramStatusStore`, including
//! the released-parser report adaptation) lives in `lyrel-core` next to the
//! `rio-vt` dependency; the pure wire values both sides agree on live here.
//! Nothing here parses an OSC sequence again or re-implements the protocol's
//! byte/id/text limits: the store applies the remaining state semantics to
//! already-validated reports and projects [`ProgramRecord`] values whose serde
//! layout is part of the wire contract.
//!
//! Consumers receive a [`ProgramSnapshot`] on attach and [`ProgramDelta`]
//! values for ordered live changes; a [`ProgramStatusClient`] applies either
//! frame without guessing a version from payload contents.
//!
//! Projection frames serialize for the wire but are not persisted. The
//! store's own serialized form is the existing transient in-memory daemon
//! handoff record (see `lyrel-core`), which must preserve the session
//! identity, both counters, the recency high-water mark, the explicit `app`
//! payload, and the records.
//!
//! [`plain_program_text`] is the display-side sanitizer for program-origin
//! text. It intentionally does not modify what the store retained.

use std::collections::{BTreeMap, BTreeSet};

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
    /// The daemon-side store's construction seam (`lyrel-core`): it resolves
    /// the effective `app` and projects one stored record. Field order matches
    /// the wire layout; the serde form is unchanged.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        state: ProgramState,
        kind: Option<BlockedKind>,
        app: Option<String>,
        title: Option<String>,
        message: Option<String>,
        progress: Option<u8>,
        updated: u64,
        semantic_version: u64,
        result_version: u64,
    ) -> Self {
        Self {
            id,
            state,
            kind,
            app,
            title,
            message,
            progress,
            updated,
            semantic_version,
            result_version,
        }
    }

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
    /// The daemon-side store's construction seam (`lyrel-core`); the serde
    /// form is unchanged.
    pub fn new(
        session_id: String,
        semantic_version: u64,
        result_version: u64,
        records: Vec<ProgramRecord>,
    ) -> Self {
        Self {
            session_id,
            semantic_version,
            result_version,
            records,
        }
    }

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
    /// The daemon-side store's construction seam (`lyrel-core`); the serde
    /// form is unchanged.
    pub fn new(
        session_id: String,
        semantic_version: u64,
        result_version: u64,
        changed: Vec<ProgramRecord>,
        removed: Vec<String>,
    ) -> Self {
        Self {
            session_id,
            semantic_version,
            result_version,
            changed,
            removed,
        }
    }

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

fn state_rank(state: ProgramState) -> u8 {
    match state {
        ProgramState::Blocked => 0,
        ProgramState::Error => 1,
        ProgramState::Working => 2,
        ProgramState::Done => 3,
        ProgramState::Idle => 4,
    }
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
