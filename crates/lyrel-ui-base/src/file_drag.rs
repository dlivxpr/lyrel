//! The payload a remote file-tree row drags with it — a path on that remote,
//! which nothing on this machine can open. Defined here, between the tree and
//! the terminal, so neither side imports the other; the drop itself is still
//! handled by the terminal's clipboard/presentation code, which turns it into
//! a paste of the paths.

use std::path::PathBuf;

/// A row dragged out of a remote Files tree. Payload only: no host, no
/// resolution — the dropping pane knows whose paths it accepts.
#[derive(Clone)]
pub struct RemotePathDrag {
    pub path: PathBuf,
}
