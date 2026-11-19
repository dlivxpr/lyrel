//! The read-only SCM snapshot seam lives in `lyrel-ui-base`; this module is
//! the root facade: it re-exports the query surface and installs the shell
//! implementation that reads the workbench's snapshot globals.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::App;

use lyrel_core::core::git::GitStatus;
use lyrel_core::core::git::status::StatusIndex;
use lyrel_ui_base::scm_snapshot::ScmSnapshotSource;

use crate::ui::host_ops::HostId;

pub use lyrel_ui_base::scm_snapshot::{GIT_RETRY_GAP, OPPORTUNISTIC_GIT_GAP, epoch, index_of};

/// Install the snapshot source the seam forwards to. Startup calls this once;
/// tests that install the controllers directly call it too.
pub(crate) fn install_scm_snapshot_source(cx: &mut App) {
    lyrel_ui_base::scm_snapshot::install_scm_snapshot_source(cx, Arc::new(ShellScmSnapshots));
}

struct ShellScmSnapshots;

impl ScmSnapshotSource for ShellScmSnapshots {
    fn index_of(&self, cx: &App, host: HostId, root: &Path) -> Option<Arc<StatusIndex>> {
        cx.try_global::<crate::ui::scm::data::ScmData>()?
            .index_for(host, root)
    }

    fn epoch(&self, cx: &App, host: HostId, root: &Path) -> u64 {
        cx.try_global::<crate::ui::scm::data::ScmData>()
            .map(|data| data.epoch(host, root))
            .unwrap_or(0)
    }

    fn badge_status(&self, cx: &App, host: HostId, cwd: &Path) -> Option<GitStatus> {
        cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()?
            .status_for(host, cwd)
    }

    fn repo_root_for(&self, cx: &App, host: HostId, cwd: &Path) -> Option<PathBuf> {
        cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()?
            .repo_root_for(host, cwd)
            .map(Path::to_path_buf)
    }

    fn retry_due(&self, cx: &App, host: HostId, cwd: &Path) -> bool {
        cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()
            .is_some_and(|cache| cache.retry_due(host, cwd, GIT_RETRY_GAP))
    }

    fn root_recheck_due(&self, cx: &App, host: HostId, cwd: &Path) -> bool {
        cx.try_global::<crate::ui::scm::status_cache::GitStatusCache>()
            .is_some_and(|cache| cache.root_recheck_due(host, cwd, GIT_RETRY_GAP))
    }
}
