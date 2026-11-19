//! The read-only seam over the source-control snapshots.
//!
//! Everything here answers a question about cached SCM state without touching
//! the controller: no claims, no bumps, no notifies. Renders and the
//! terminal's per-tick poll gates read through this module so the production
//! terminal never imports the SCM controller.
//!
//! The snapshot globals themselves live in the workbench (`ScmData`,
//! `GitStatusCache`), above this crate; the workbench installs one
//! `ScmSnapshotSource` at startup and these reads forward to it. A caller
//! that never installed the source — a view unit test, say — gets the same
//! answers a missing controller always gave: `None`, `0`, `false`.
//!
//! The rules that keep this cheap and quiet:
//!
//! - Nothing here allocates per call beyond the returned value; the hot
//!   questions — "is a retry due?", "what epoch is this root at?" — are
//!   borrows of the globals as they stand.
//! - Nothing here notifies. A changed snapshot reaches the screen through
//!   the controller's own change-gated repaint paths, never through this
//!   seam.
//!
//! The write-side counterparts of these reads live exclusively in the SCM
//! controller and the shell wiring that answers a pane's typed events.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, Global};

use lyrel_core::core::git::GitStatus;
use lyrel_core::core::git::status::StatusIndex;
use lyrel_core::host::HostId;

/// How long a pane with an unknown or negative repository answer waits before
/// asking again. Shared by the pane-side read gate ([`retry_due`]) and the
/// controller's `Retry` claim, so a gate that says "due" always meets a claim
/// that agrees.
pub const GIT_RETRY_GAP: Duration = Duration::from_secs(10);

/// The budget an opportunistic badge probe waits out before the controller
/// will run another one for the same cwd.
pub const OPPORTUNISTIC_GIT_GAP: Duration = Duration::from_millis(1500);

/// The workbench's read-side answer to SCM snapshot questions. Implemented
/// once, above this crate, where the snapshot globals live; installed with
/// [`install_scm_snapshot_source`].
pub trait ScmSnapshotSource: 'static {
    /// The per-path decoration index for a repository. Its `Arc` identity is
    /// stable until the underlying working-tree status changes.
    fn index_of(&self, cx: &App, host: HostId, root: &Path) -> Option<Arc<StatusIndex>>;
    /// What `root`'s cached snapshot was read at — the larger of the
    /// repository's own epoch and the subscription generation guarding late
    /// landings.
    fn epoch(&self, cx: &App, host: HostId, root: &Path) -> u64;
    /// The `+N −M` badge a pane's tab wears: the cached answer for `cwd`'s
    /// repository, cheap enough to read on a render.
    fn badge_status(&self, cx: &App, host: HostId, cwd: &Path) -> Option<GitStatus>;
    /// The working tree `cwd` sits in, once the cheap probe has found out.
    fn repo_root_for(&self, cx: &App, host: HostId, cwd: &Path) -> Option<PathBuf>;
    /// Whether a badge probe for `cwd` would be accepted right now.
    fn retry_due(&self, cx: &App, host: HostId, cwd: &Path) -> bool;
    /// Whether a root revalidation for `cwd` is due.
    fn root_recheck_due(&self, cx: &App, host: HostId, cwd: &Path) -> bool;
}

struct ScmSnapshotSourceGlobal(Arc<dyn ScmSnapshotSource>);

impl Global for ScmSnapshotSourceGlobal {}

/// Install the workbench's snapshot source. Called once at startup; test
/// harnesses that install the controllers directly install it too.
pub fn install_scm_snapshot_source(cx: &mut App, source: Arc<dyn ScmSnapshotSource>) {
    cx.set_global(ScmSnapshotSourceGlobal(source));
}

fn source(cx: &App) -> Option<&dyn ScmSnapshotSource> {
    cx.try_global::<ScmSnapshotSourceGlobal>()
        .map(|global| &*global.0)
}

/// The per-path decoration index for a repository. Its `Arc` identity is
/// stable until the underlying working-tree status changes.
pub fn index_of(cx: &App, host: HostId, root: &Path) -> Option<Arc<StatusIndex>> {
    source(cx)?.index_of(cx, host, root)
}

/// What `root`'s cached snapshot was read at — the larger of the repository's
/// own epoch and the subscription generation that guards late landings. A
/// gutter's local snapshot compares this against the epoch it fetched at;
/// it is how a stage or a checkout a frame ago asks for one re-read, not a
/// standing refresh.
pub fn epoch(cx: &App, host: HostId, root: &Path) -> u64 {
    source(cx).map(|source| source.epoch(cx, host, root)).unwrap_or(0)
}

/// The `+N −M` badge a pane's tab wears: the cached answer for `cwd`'s
/// repository, cheap enough to read on a render.
pub fn badge_status(cx: &App, host: HostId, cwd: &Path) -> Option<GitStatus> {
    source(cx)?.badge_status(cx, host, cwd)
}

/// The working tree `cwd` sits in, once the cheap probe has found out. A copy
/// rather than a borrow so link-resolution closures can hand it to a worker.
pub fn repo_root_for(cx: &App, host: HostId, cwd: &Path) -> Option<PathBuf> {
    source(cx)?.repo_root_for(cx, host, cwd)
}

/// Whether a badge probe for `cwd` would be accepted right now — the pane-side
/// half of the `Retry` transition. Read-only on purpose: this runs on every
/// poll tick, and a mutable touch of the cache wakes every global observer,
/// which would put a pane asking three times a second behind three repaints a
/// second to be told no.
pub fn retry_due(cx: &App, host: HostId, cwd: &Path) -> bool {
    source(cx).is_some_and(|source| source.retry_due(cx, host, cwd))
}

/// Whether a root revalidation for `cwd` is due — the pane-side gate that
/// keeps the recheck from emitting an event (and the controller from claiming
/// a check) on every quiet tick. Mirrors the early-out of the controller's
/// root check without recording anything.
pub fn root_recheck_due(cx: &App, host: HostId, cwd: &Path) -> bool {
    source(cx)
        .is_some_and(|source| source.root_recheck_due(cx, host, cwd))
}
