//! The read-only seam over the source-control snapshots.
//!
//! Everything here answers a question about cached SCM state without touching
//! the controller: no claims, no bumps, no notifies. Renders and the
//! terminal's per-tick poll gates read through this module so the production
//! terminal never imports the SCM controller ([`crate::ui::scm::data`] /
//! [`crate::ui::scm::status_cache`]).
//!
//! The rules that keep this cheap and quiet:
//!
//! - Every lookup goes through `try_global`: a caller that never installed
//!   the controller (a view unit test, say) gets `None`, never a default
//!   write, and no observer wakeups.
//! - Nothing here allocates per call beyond the returned value; the hot
//!   questions — "is a retry due?", "what epoch is this root at?" — are
//!   borrows of the globals as they stand.
//! - Nothing here notifies. A changed snapshot reaches the screen through
//!   the controller's own change-gated repaint paths, never through this
//!   seam.
//!
//! The write-side counterparts of these reads live exclusively in the SCM
//! controller and the shell wiring that answers a pane's typed events
//! ([`crate::terminal::view::ScmStatusRefreshRequested`] and friends).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::core::git::GitStatus;
use crate::core::git::status::StatusIndex;
use crate::ui::host_ops::HostId;

/// How long a pane with an unknown or negative repository answer waits before
/// asking again. Shared by the pane-side read gate ([`retry_due`]) and the
/// controller's `Retry` claim, so a gate that says "due" always meets a claim
/// that agrees.
pub const GIT_RETRY_GAP: Duration = Duration::from_secs(10);

/// The budget an opportunistic badge probe ([`crate::terminal::view::GitRefresh::Opportunistic`])
/// waits out before the controller will run another one for the same cwd.
pub const OPPORTUNISTIC_GIT_GAP: Duration = Duration::from_millis(1500);

/// The per-path decoration index for a repository. Its `Arc` identity is
/// stable until the underlying working-tree status changes.
pub fn index_of(cx: &gpui_kit::App, host: HostId, root: &Path) -> Option<Arc<StatusIndex>> {
    cx.try_global::<crate::ui::scm::data::ScmData>()?
        .index_for(host, root)
}

/// What `root`'s cached snapshot was read at — the larger of the repository's
/// own epoch and the subscription generation that guards late landings. A
/// gutter's local snapshot compares this against the epoch it fetched at;
/// it is how a stage or a checkout a frame ago asks for one re-read, not a
/// standing refresh.
pub fn epoch(cx: &gpui_kit::App, host: HostId, root: &Path) -> u64 {
    cx.try_global::<crate::ui::scm::data::ScmData>()
        .map(|data| data.epoch(host, root))
        .unwrap_or(0)
}

/// The `+N −M` badge a pane's tab wears: the cached answer for `cwd`'s
/// repository, cheap enough to read on a render.
pub fn badge_status(cx: &gpui_kit::App, host: HostId, cwd: &Path) -> Option<GitStatus> {
    cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()?
        .status_for(host, cwd)
}

/// The working tree `cwd` sits in, once the cheap probe has found out. A copy
/// rather than a borrow so link-resolution closures can hand it to a worker.
pub fn repo_root_for(cx: &gpui_kit::App, host: HostId, cwd: &Path) -> Option<PathBuf> {
    cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()?
        .repo_root_for(host, cwd)
        .map(Path::to_path_buf)
}

/// Whether a badge probe for `cwd` would be accepted right now — the pane-side
/// half of the `Retry` transition. Read-only on purpose: this runs on every
/// poll tick, and a mutable touch of the cache wakes every global observer,
/// which would put a pane asking three times a second behind three repaints a
/// second to be told no.
pub fn retry_due(cx: &gpui_kit::App, host: HostId, cwd: &Path) -> bool {
    cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()
        .is_some_and(|cache| cache.retry_due(host, cwd, GIT_RETRY_GAP))
}

/// Whether a root revalidation for `cwd` is due — the pane-side gate that
/// keeps `recheck_git_root` from emitting an event (and the controller from
/// claiming a check) on every quiet tick. Mirrors the early-out of the
/// controller's `begin_root_check` without recording anything.
pub fn root_recheck_due(cx: &gpui_kit::App, host: HostId, cwd: &Path) -> bool {
    cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()
        .is_some_and(|cache| cache.root_recheck_due(host, cwd, GIT_RETRY_GAP))
}
