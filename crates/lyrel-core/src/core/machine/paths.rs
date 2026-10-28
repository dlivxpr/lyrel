//! Instance-local paths and daemon-startup adoption of existing data.

use std::io;
use std::path::{Path, PathBuf};

pub const MACHINE_FILE: &str = "machine.json";

pub const APPEARANCE_FILE: &str = "appearance.json";

pub const DATA_DIR_ENV: &str = "LYREL_DATA_DIR";

pub fn default_machine_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join(MACHINE_FILE))
}

pub fn appearance_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join(APPEARANCE_FILE))
}

/// Where the machine tree lives: the config directory, unless a test pins one
/// with [`DATA_DIR_ENV`].
///
/// It used to be the XDG data directory, resolved from `HOME` alone. That made
/// the tree the one piece of an instance that `--config-dir` did not move, so
/// two lyrels pointed at different config directories — each holding its own
/// `daemon.lock`, each certain it was the only server — still co-owned one
/// `machine.json`. The machine store writes the document whole, so the
/// second one to flush replaced the first one's workspaces with its own, and
/// the next daemon to start read the survivor's tree as the machine's. A tree
/// that comes back empty is not distinguishable from a machine that really has
/// nothing on it, so the GUI does what an empty tree means (see
/// `tree_sync::on_workspace_deleted`) and forgets the workspaces for good.
///
/// The lock and the thing it protects have to be keyed alike. Everything else
/// an instance owns — `views.json`, the scrollback, the history, both sockets,
/// the pidfile, the lock — is already keyed by the config directory; the tree
/// and the appearance hint were the last two files that were not.
fn data_dir() -> io::Result<PathBuf> {
    if let Some(explicit) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    crate::core::config::config_dir_path().ok_or_else(|| {
        io::Error::other(format!(
            "no config directory to place {MACHINE_FILE} in; set {DATA_DIR_ENV}"
        ))
    })
}

/// Where [`data_dir`] pointed before it followed the config directory.
///
/// Deliberately still resolved the old way, `LYREL_DATA_DIR` included: this
/// answers "where would the build the user just upgraded from have put it",
/// and that build read the environment, not the config directory.
fn legacy_data_dir() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(explicit));
    }
    #[cfg(not(windows))]
    let base = env_dir("XDG_DATA_HOME")
        .or_else(|| env_dir("HOME").map(|h| h.join(".local").join("share")));
    #[cfg(windows)]
    let base = env_dir("LOCALAPPDATA")
        .or_else(|| env_dir("USERPROFILE").map(|h| h.join(".local").join("share")));

    base.map(|b| b.join("lyrel"))
}

/// Carry an upgrading install's tree and appearance hint over to the config
/// directory.
///
/// Moving the path without this would lose every workspace on the machine at
/// the moment of upgrade, which is the failure this whole change exists to
/// stop.
///
/// Called by the daemon at startup, before it opens the store, and by nothing
/// else. The daemon is the tree's writer, so it is the process entitled to move
/// it; and a path getter that touches the disk as a side effect is one no test
/// can call without putting the developer's own tree at risk — which is the
/// accident this whole change is about.
///
/// The appearance hint rides along, and it does have a second writer: the GUI
/// records it whenever it applies a theme ([`super::note_appearance`]). Both writers
/// land their file with a rename, so the worst a collision costs is one of two
/// booleans, and the next theme the GUI applies overwrites it either way.
pub fn adopt_legacy_data_dir() {
    adopt_into(
        data_dir().ok().as_deref(),
        legacy_data_dir().as_deref(),
        this_is_the_machines_instance(),
    );
}

/// [`adopt_legacy_data_dir`] with every process-wide answer already looked up,
/// so a test can state the situation instead of arranging one.
fn adopt_into(current: Option<&Path>, legacy: Option<&Path>, machines_instance: bool) {
    let (Some(current), Some(legacy)) = (current, legacy) else {
        return;
    };
    // The same directory under two names is not a migration. This is also what
    // makes `LYREL_DATA_DIR` a no-op: both getters answer with it.
    if legacy == current {
        return;
    }
    if !machines_instance {
        decline_legacy(legacy);
        return;
    }
    for file in [MACHINE_FILE, APPEARANCE_FILE] {
        adopt_legacy_file(&legacy.join(file), &current.join(file));
    }
}

/// Whether this process is the machine's lyrel rather than an instance somebody
/// pointed somewhere else.
///
/// There is one legacy tree and there can be any number of instances, so "who
/// inherits it" has to have exactly one answer, and it cannot be "whoever
/// starts first". Every build before this change read the tree that
/// [`legacy_data_dir`] names, so the instance entitled to it is the one still
/// running out of the config directory this machine resolves to on its own
/// ([`config::machine_config_dir`](crate::core::config::machine_config_dir)) —
/// `$LYREL_CONFIG_DIR` when the box sets one, `$HOME`'s otherwise. Anything
/// aimed elsewhere is a second lyrel by definition, and a second lyrel starts on
/// an empty tree.
///
/// Comparing paths rather than asking whether `--config-dir` was passed is what
/// makes the ordinary install work: [`daemon::spawn`](crate::daemon::spawn)
/// hands the daemon an explicit `--config-dir` every time, its own resolved
/// directory included, so "was the flag given" is true for everybody and would
/// decline for everybody.
///
/// Deciding by start order instead would let a second instance rename the tree
/// out from under the first — this bug wearing a different hat, the primary
/// coming up owning nothing and every workspace with no window on it gone. It
/// would also fire in our own test suite, where `routed_pane` and friends
/// launch a real `lyrel-server --config-dir <TempDir>` under the developer's own
/// `HOME`: a start-order rule moves the developer's real `machine.json` into a
/// scratch directory and deletes it with the `TempDir`.
///
/// Counting `$LYREL_CONFIG_DIR` as the machine's is what keeps remote hosts
/// upgrading: a remote `lyrel-server` is launched with no `--config-dir` and
/// finds its config directory exactly this way (see
/// `daemon::remote_link::remote_control_socket`), so a rule written against
/// `$HOME` alone would strand the tree on every box that names one.
fn this_is_the_machines_instance() -> bool {
    is_the_machines_instance(
        crate::core::config::config_dir_path().as_deref(),
        crate::core::config::machine_config_dir().as_deref(),
    )
}

fn is_the_machines_instance(current: Option<&Path>, machines: Option<&Path>) -> bool {
    matches!((current, machines), (Some(c), Some(m)) if c == m)
}

/// Say where the tree was left when this instance is not the one entitled to
/// take it, so "my workspaces are gone" has an answer in the log rather than
/// only in this comment.
fn decline_legacy(legacy: &Path) {
    let file = legacy.join(MACHINE_FILE);
    if !file.exists() {
        return;
    }
    log::info!(
        "leaving {} where it is: this instance runs on a config directory of its own, and \
         {LEGACY_NOTE}. Move it in by hand if this is the instance that should have it.",
        file.display()
    );
}

/// The destination existing at all is the whole guard. It means some newer run
/// already owns this file, and the legacy copy beside it is stale — a build
/// from before this change, started once since the move, writing where it still
/// believes the tree lives. Overwriting would hand that stale tree back.
///
/// Two processes migrating at once is safe for the same reason they cannot both
/// win: they rename the same source, so the loser's rename fails with the
/// source already gone — see [`adopt_by_copy`], which tells that apart from a
/// rename that failed with something still to carry over.
fn adopt_legacy_file(legacy: &Path, current: &Path) {
    if current.exists() || !legacy.exists() {
        return;
    }
    if let Some(parent) = current.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        log::warn!(
            "could not create {} to receive {}: {e}",
            parent.display(),
            legacy.display()
        );
        return;
    }
    match std::fs::rename(legacy, current) {
        Ok(()) => log::info!(
            "moved {} to {} ({LEGACY_NOTE})",
            legacy.display(),
            current.display()
        ),
        Err(e) => adopt_by_copy(legacy, current, &e),
    }
}

/// The rename did not go through. Two reasons to tell apart, because only one
/// of them is a problem.
///
/// The source being gone is the concurrent-migration race: another process
/// renamed it away between the check above and here, and its result is the one
/// this call wanted anyway. Reporting that as a failure — or copying over it —
/// would turn the one benign race into noise in the log.
///
/// Otherwise the usual reason is that the two directories are on different
/// filesystems, where `rename` refuses and a copy is the only way over. Writing
/// it with `create_new` keeps "never overwrite what is already there" true
/// against a racing writer and not merely against the `exists` check, which by
/// now is several syscalls stale. The original stays: it costs a file nobody
/// reads, and losing the tree to a half-finished move is the one outcome worth
/// ruling out.
fn adopt_by_copy(legacy: &Path, current: &Path, why: &io::Error) {
    if !legacy.exists() {
        log::debug!(
            "{} was carried over by another process ({why})",
            legacy.display()
        );
        return;
    }
    match copy_new(legacy, current) {
        Ok(()) => log::info!(
            "copied {} to {} ({LEGACY_NOTE}); the original was left in place",
            legacy.display(),
            current.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => log::debug!(
            "{} already has one; leaving {} where it is",
            current.display(),
            legacy.display()
        ),
        Err(e) => log::warn!(
            "could not move {} to {} ({why}), nor copy it: {e}",
            legacy.display(),
            current.display()
        ),
    }
}

/// Copy `from` onto a `to` that must not already exist, leaving nothing behind
/// if the write does not finish: a truncated tree is a tree, and the next
/// startup would adopt nothing over it.
///
/// Read whole rather than streamed so a failure lands before the destination is
/// created, and private because that is how both files are written
/// ([`crate::core::config::write_atomic_private`]) — `fs::copy` would carry the
/// legacy mode over, which is the same thing only as long as the legacy file
/// was ours.
fn copy_new(from: &Path, to: &Path) -> io::Result<()> {
    use std::io::Write as _;

    let bytes = std::fs::read(from)?;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        open.mode(0o600);
    }
    let mut file = open.open(to)?;
    if let Err(e) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(to);
        return Err(e);
    }
    Ok(())
}

const LEGACY_NOTE: &str = "the machine tree now lives beside the rest of the config directory";

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests;
