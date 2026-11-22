//! Bounded batch session deletion, one prepare/execute workflow shared by
//! every provider that gains a verified deletion path (#367 Hermes first;
//! Kimi/OpenCode/pi/OMP reuse the same pair in #368–370).
//!
//! A selection can mix scopes and working directories; each [`SessionBatchGroup`]
//! carries its own exact native store identity (`cwd`, `launch_argv`, durable
//! `scope`), so execution can never act on a broader store than the one the
//! confirmation bound. Prepare revalidates every candidate on its owning Host
//! and returns the exact native cascade per target; Execute fences that
//! captured set inside the provider's own write transaction and reports
//! deleted/skipped/failed per target, never silently narrowing or broadening.

use super::{
    CLIAgent, Host, SessionErrorKind, SessionOperationError, SessionScope, failure, guard_off_ui,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One prepare/execute request pair bounds the whole batch: a select-all
/// that overflows this stays a client-side refusal instead of one unbounded
/// native transaction.
pub const SESSION_BATCH_CAP: usize = 200;

/// One store-scoped slice of the selection. `cwd`/`launch_argv`/`scope` are
/// the same selectors the discovery layer preserved; Execute answers the
/// identical grouping Prepare returned, in request order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchPrepareGroup {
    pub cwd: PathBuf,
    pub launch_argv: Vec<String>,
    pub scope: SessionScope,
    /// The stored session ids the user selected in this group, in the
    /// selection's order.
    pub source_ids: Vec<String>,
}

/// The exact target one confirmation binds: the stored session id plus the
/// provider-native cascade Prepare computed for it. Adapters revalidate this
/// set inside their write transaction, so drift is a skipped row, never a
/// broader deletion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchConfirmation {
    pub source_id: String,
    pub expected_delete_ids: Vec<String>,
}

/// The execute-side group: same store identity as its prepare group, with
/// the confirmed targets replacing the candidate list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchExecuteGroup {
    pub cwd: PathBuf,
    pub launch_argv: Vec<String>,
    pub scope: SessionScope,
    pub confirmations: Vec<SessionBatchConfirmation>,
}

/// The one request enum every provider's batch rides under: Prepare plans,
/// Execute removes exactly what was confirmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionBatchRequest {
    Prepare(Box<SessionBatchPrepare>),
    Execute(Box<SessionBatchExecute>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchPrepare {
    pub expected_host: u64,
    pub agent: CLIAgent,
    pub groups: Vec<SessionBatchPrepareGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchExecute {
    pub expected_host: u64,
    pub agent: CLIAgent,
    pub groups: Vec<SessionBatchExecuteGroup>,
}

impl SessionBatchRequest {
    pub fn expected_host(&self) -> u64 {
        match self {
            SessionBatchRequest::Prepare(request) => request.expected_host,
            SessionBatchRequest::Execute(request) => request.expected_host,
        }
    }

    pub fn agent(&self) -> CLIAgent {
        match self {
            SessionBatchRequest::Prepare(request) => request.agent,
            SessionBatchRequest::Execute(request) => request.agent,
        }
    }

    /// Every candidate/confirmed source id across all groups, in request
    /// order — the bounded-ness check both sides run before anything native.
    pub fn source_ids(&self) -> Vec<&str> {
        match self {
            SessionBatchRequest::Prepare(request) => request
                .groups
                .iter()
                .flat_map(|group| group.source_ids.iter().map(String::as_str))
                .collect(),
            SessionBatchRequest::Execute(request) => request
                .groups
                .iter()
                .flat_map(|group| {
                    group
                        .confirmations
                        .iter()
                        .map(|confirmation| confirmation.source_id.as_str())
                })
                .collect(),
        }
    }
}

/// Why one target is ineligible at prepare or was left intact/failed at
/// execute. Each kind is an actionable sentence in the UI, not a code the
/// user has to look up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionBatchReasonKind {
    /// A live writer holds the session: an open Lyrel pane at the same
    /// identity, or a provider-native writer/turn lease at execute time.
    Active,
    /// The stored record changed or disappeared since the listing/prepare;
    /// the confirmation's exact set no longer matches the native store.
    Stale,
    /// This provider has no verified deletion path yet (#368–370 land
    /// theirs behind the same pair).
    UnsupportedProvider,
    /// The exact native store could not be confirmed, so no verified
    /// deletion can say what it would remove.
    UnconfirmedStore,
    /// The provider's native operation failed; the message carries the
    /// provider's own wording.
    Provider,
}

/// A per-target reason with the provider's or transport's own wording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchReason {
    pub kind: SessionBatchReasonKind,
    pub message: String,
}

impl SessionBatchReason {
    pub fn new(kind: SessionBatchReasonKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// The per-target verdict inside one group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionBatchStatus {
    /// Prepare: store identity and cascade revalidated; `delete_ids` is the
    /// exact native scope the confirmation shows.
    Eligible,
    /// Prepare: not deletable now; `reason` says why and how to fix it.
    Ineligible,
    /// Execute: the confirmed cascade was removed.
    Deleted,
    /// Execute: left intact — revalidation drifted or a writer lease held it.
    Skipped,
    /// Execute: the provider reported a failure for this target.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchItem {
    pub source_id: String,
    pub status: SessionBatchStatus,
    /// The exact native ids this item bound at prepare. Empty for an
    /// ineligible prepare answer; the confirmed set for a deleted/skipped
    /// execute answer, so the report can name precisely what was in scope.
    pub delete_ids: Vec<String>,
    /// Actionable reason for Ineligible/Skipped/Failed.
    pub reason: Option<SessionBatchReason>,
}

impl SessionBatchItem {
    pub fn eligible(source_id: String, delete_ids: Vec<String>) -> Self {
        Self {
            source_id,
            status: SessionBatchStatus::Eligible,
            delete_ids,
            reason: None,
        }
    }

    pub fn refused(
        source_id: String,
        status: SessionBatchStatus,
        reason: SessionBatchReason,
    ) -> Self {
        Self {
            source_id,
            status,
            delete_ids: Vec::new(),
            reason: Some(reason),
        }
    }
}

/// One group's answers, in the request's source order — the aggregate
/// report concatenates groups without reordering.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchGroupResult {
    pub items: Vec<SessionBatchItem>,
}

impl SessionBatchGroupResult {
    pub fn eligible(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == SessionBatchStatus::Eligible)
            .count()
    }

    pub fn status_count(&self, status: SessionBatchStatus) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == status)
            .count()
    }
}

/// The one reply enum under the batch pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionBatchReply {
    Prepare(SessionBatchPrepareResult),
    Execute(SessionBatchExecuteResult),
}

impl SessionBatchReply {
    pub fn group_results(&self) -> &[SessionBatchGroupResult] {
        match self {
            SessionBatchReply::Prepare(result) => &result.groups,
            SessionBatchReply::Execute(result) => &result.groups,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchPrepareResult {
    /// One result per requested group, in request order.
    pub groups: Vec<SessionBatchGroupResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBatchExecuteResult {
    /// One result per executed group, in request order.
    pub groups: Vec<SessionBatchGroupResult>,
}

impl SessionBatchExecuteResult {
    /// Aggregate across every group, so a mixed-scope selection reports one
    /// accurate deleted/skipped/failed tally.
    pub fn totals(&self) -> SessionBatchTotals {
        let mut totals = SessionBatchTotals::default();
        for group in &self.groups {
            totals.deleted += group.status_count(SessionBatchStatus::Deleted);
            totals.skipped += group.status_count(SessionBatchStatus::Skipped);
            totals.failed += group.status_count(SessionBatchStatus::Failed);
        }
        totals
    }

    /// Every native id actually removed, across groups, in report order.
    pub fn deleted_ids(&self) -> Vec<&str> {
        self.groups
            .iter()
            .flat_map(|group| {
                group
                    .items
                    .iter()
                    .filter(|item| item.status == SessionBatchStatus::Deleted)
                    .flat_map(|item| item.delete_ids.iter().map(String::as_str))
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionBatchTotals {
    pub deleted: usize,
    pub skipped: usize,
    pub failed: usize,
}

fn valid_source_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Host-side entry for the pair: route/connection/identity revalidation for
/// the whole request, then per-agent dispatch. Every provider adapter runs
/// on this Host — the GUI never interprets a native store itself.
pub(crate) fn run<H: Host + ?Sized>(
    host: &H,
    request: &SessionBatchRequest,
) -> Result<SessionBatchReply, SessionOperationError> {
    guard_off_ui();
    if host.id().0 != request.expected_host() {
        return Err(failure(
            SessionErrorKind::WrongHost,
            "session batch belongs to another host",
        ));
    }
    if !host.is_connected() {
        return Err(failure(
            SessionErrorKind::Connection,
            "session batch host is disconnected",
        ));
    }
    if request.source_ids().len() > SESSION_BATCH_CAP {
        return Err(failure(
            SessionErrorKind::Unsupported,
            format!("session batch is limited to {SESSION_BATCH_CAP} targets at once"),
        ));
    }
    let groups = match request {
        SessionBatchRequest::Prepare(request) => request.groups.len(),
        SessionBatchRequest::Execute(request) => request.groups.len(),
    };
    if groups == 0 {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "session batch has no targets",
        ));
    }
    if request.source_ids().iter().any(|id| !valid_source_id(id)) {
        return Err(failure(
            SessionErrorKind::Identity,
            "invalid session identity in batch",
        ));
    }
    // The executing Host revalidates every group's working directory: a
    // selection planned against a deleted directory never reaches a provider.
    let directories_ok = match request {
        SessionBatchRequest::Prepare(request) => request
            .groups
            .iter()
            .all(|group| directory_valid(host, &group.cwd)),
        SessionBatchRequest::Execute(request) => request
            .groups
            .iter()
            .all(|group| directory_valid(host, &group.cwd)),
    };
    if !directories_ok {
        return Err(failure(
            SessionErrorKind::Directory,
            "session batch requires an absolute existing directory on its Host",
        ));
    }
    match request.agent() {
        CLIAgent::Hermes => super::hermes::batch(request),
        // #368–370 plug their verified adapters in here. Until then every
        // other provider answers ineligible with an explicit reason — the
        // action stays visibly unsupported instead of silently no-op'ing.
        other => Ok(unsupported_reply(request, other)),
    }
}

fn directory_valid<H: Host + ?Sized>(host: &H, cwd: &std::path::Path) -> bool {
    host.is_absolute(cwd) && host.stat(cwd).is_ok_and(|meta| meta.is_dir)
}

fn unsupported_reply(request: &SessionBatchRequest, agent: CLIAgent) -> SessionBatchReply {
    let reason = SessionBatchReason::new(
        SessionBatchReasonKind::UnsupportedProvider,
        format!(
            "{} sessions have no verified deletion path in this Lyrel version",
            agent.display_name()
        ),
    );
    match request {
        SessionBatchRequest::Prepare(request) => {
            SessionBatchReply::Prepare(SessionBatchPrepareResult {
                groups: request
                    .groups
                    .iter()
                    .map(|group| SessionBatchGroupResult {
                        items: group
                            .source_ids
                            .iter()
                            .map(|id| {
                                SessionBatchItem::refused(
                                    id.clone(),
                                    SessionBatchStatus::Ineligible,
                                    reason.clone(),
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            })
        }
        SessionBatchRequest::Execute(request) => {
            SessionBatchReply::Execute(SessionBatchExecuteResult {
                groups: request
                    .groups
                    .iter()
                    .map(|group| SessionBatchGroupResult {
                        items: group
                            .confirmations
                            .iter()
                            .map(|confirmation| {
                                SessionBatchItem::refused(
                                    confirmation.source_id.clone(),
                                    SessionBatchStatus::Skipped,
                                    reason.clone(),
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(source_ids: &[&str]) -> SessionBatchPrepareGroup {
        SessionBatchPrepareGroup {
            cwd: PathBuf::from("/w"),
            launch_argv: vec!["hermes".into(), "chat".into()],
            scope: SessionScope::default(),
            source_ids: source_ids.iter().map(|id| (*id).to_string()).collect(),
        }
    }

    #[test]
    fn request_round_trips_under_one_pair_shape() {
        let request = SessionBatchRequest::Prepare(Box::new(SessionBatchPrepare {
            expected_host: 7,
            agent: CLIAgent::Hermes,
            groups: vec![group(&["a", "b"]), group(&["c"])],
        }));
        let json = serde_json::to_string(&request).unwrap();
        let decoded: SessionBatchRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(decoded.expected_host(), 7);
        assert_eq!(decoded.agent(), CLIAgent::Hermes);
        assert_eq!(decoded.source_ids(), ["a", "b", "c"]);
    }

    #[test]
    fn execute_totals_aggregate_across_groups_in_order() {
        let item = |source_id: &str, status: SessionBatchStatus| SessionBatchItem {
            source_id: source_id.into(),
            status,
            delete_ids: match status {
                SessionBatchStatus::Deleted => vec![source_id.into()],
                _ => Vec::new(),
            },
            reason: None,
        };
        let result = SessionBatchExecuteResult {
            groups: vec![
                SessionBatchGroupResult {
                    items: vec![
                        item("a", SessionBatchStatus::Deleted),
                        item("b", SessionBatchStatus::Skipped),
                    ],
                },
                SessionBatchGroupResult {
                    items: vec![
                        item("c", SessionBatchStatus::Deleted),
                        item("d", SessionBatchStatus::Failed),
                    ],
                },
            ],
        };
        let totals = result.totals();
        assert_eq!((totals.deleted, totals.skipped, totals.failed), (2, 1, 1));
        assert_eq!(result.deleted_ids(), ["a", "c"]);
    }

    #[test]
    fn unsupported_provider_marks_every_target_ineligible_in_request_order() {
        let request = SessionBatchRequest::Prepare(Box::new(SessionBatchPrepare {
            expected_host: 0,
            agent: CLIAgent::Kimi,
            groups: vec![group(&["k1", "k2"])],
        }));
        let reply = super::unsupported_reply(&request, CLIAgent::Kimi);
        let groups = reply.group_results();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
        assert!(groups[0].items.iter().all(|item| {
            item.status == SessionBatchStatus::Ineligible
                && item.reason.as_ref().is_some_and(|reason| {
                    reason.kind == SessionBatchReasonKind::UnsupportedProvider
                })
        }));
        assert_eq!(
            groups[0]
                .items
                .iter()
                .map(|item| item.source_id.as_str())
                .collect::<Vec<_>>(),
            ["k1", "k2"]
        );
    }

    #[test]
    fn group_results_keep_per_group_shape() {
        let mut result = SessionBatchPrepareResult::default();
        result.groups.push(SessionBatchGroupResult {
            items: vec![
                SessionBatchItem::eligible("a".into(), vec!["a".into(), "a.child".into()]),
                SessionBatchItem::refused(
                    "b".into(),
                    SessionBatchStatus::Ineligible,
                    SessionBatchReason::new(SessionBatchReasonKind::Stale, "gone"),
                ),
            ],
        });
        assert_eq!(result.groups[0].eligible(), 1);
        assert_eq!(result.groups[0].items[0].delete_ids, ["a", "a.child"]);
    }
}
