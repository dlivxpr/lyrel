//! Whole-document persistence, bounded spaced backups, and startup recovery.
//! The store supplies its already-locked snapshot and owns rollback/publication.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{MACHINE_FILE, Machine};

/// How many earlier generations of the machine tree are kept beside it.
///
/// Bounded on purpose: the tree is rewritten whole on every mutation, so an
/// unbounded history would be a new file every few seconds forever. Three is
/// what the spacing below has to work with.
const BACKUP_GENERATIONS: usize = 3;

/// How far apart those generations are taken.
///
/// The point of the spacing is that "the previous write" is worthless as a
/// backup here. The tree is written whole on every mutation, and the failure
/// worth recovering from arrives as a burst — #716 emptied a workspace with
/// nineteen `TabClose`s in a row, each of them a persist. A backup taken on
/// every write would have rolled the damage through all three generations
/// before anyone looked. Spaced out, the oldest generation is a quarter of an
/// hour of history, which is longer than any burst.
///
/// Note that "previous good" cannot mean "the last document that parsed":
/// an emptied tree parses perfectly and is exactly what you want to recover
/// *from*. Age is the only signal available here, so age is what is used.
const BACKUP_SPACING: Duration = Duration::from_secs(300);

pub(super) fn persist(path: &Path, m: &Machine) -> io::Result<()> {
    let mut doc = m.clone();
    for ws in &mut doc.workspaces {
        ws.attachment = None;
    }
    let bytes = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Before the overwrite, never after: the backup is the document about
    // to be replaced. A failure here is not a reason to stop writing the
    // new one — a machine with no backup still has to keep working.
    if let Err(e) = keep_a_generation(path, BACKUP_SPACING) {
        log::warn!(
            "could not keep an earlier generation of {}: {e}",
            path.display()
        );
    }
    crate::core::config::write_atomic_private(path, &bytes)
}

pub(super) fn load_machine(path: &Path) -> Machine {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Machine::default(),
        Err(e) => {
            log::warn!("could not read {}; quarantining it: {e}", path.display());
            crate::core::config::quarantine_by_rename(path);
            return load_a_backup(path).unwrap_or_default();
        }
    };
    match parse_machine(&text) {
        Ok(machine) => machine,
        Err(e) => {
            log::warn!("{} does not parse ({e}); quarantining it", path.display());
            crate::core::config::quarantine(path);
            load_a_backup(path).unwrap_or_default()
        }
    }
}

fn parse_machine(text: &str) -> serde_json::Result<Machine> {
    let mut machine = serde_json::from_str::<Machine>(crate::core::config::strip_bom(text))?;
    for pane in &mut machine.panes {
        pane.live = false;
    }
    Ok(machine)
}

/// The newest kept generation that still parses.
///
/// Only reached when the live document is gone or unreadable — a tree that
/// parses is always preferred to a backup, however empty it turns out to be.
/// This is the automatic half of [`keep_a_generation`]; the manual half is a
/// human copying a `.bak` over `machine.json`, which is why the files are
/// plain JSON under obvious names.
fn load_a_backup(path: &Path) -> Option<Machine> {
    (0..BACKUP_GENERATIONS).find_map(|generation| {
        let kept = backup_path(path, generation);
        let machine = parse_machine(&std::fs::read_to_string(&kept).ok()?).ok()?;
        log::warn!("recovered the machine tree from {}", kept.display());
        Some(machine)
    })
}

/// Where generation `n` of `path` is kept — `machine.json.bak` for the newest,
/// `machine.json.bak.1` and `.bak.2` behind it.
fn backup_path(path: &Path, generation: usize) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(MACHINE_FILE);
    match generation {
        0 => path.with_file_name(format!("{name}.bak")),
        n => path.with_file_name(format!("{name}.bak.{n}")),
    }
}

/// Rotates the document currently at `path` into the backup ring, if the ring's
/// newest entry is at least `spacing` old.
///
/// The live file is never renamed, only read: at every point in this function
/// `path` still holds a complete document, so a crash part-way through costs at
/// most one *backup* generation and never the tree itself. The new generation
/// lands through `write_atomic_private`, which writes a sibling temporary and
/// renames it into place — so a reader never sees a half-written `.bak` either,
/// and the 0600 the live tree is written under is the mode the copies get.
///
/// Returns `Ok(())` when there was nothing to do: no tree yet, or the newest
/// generation is younger than `spacing`.
fn keep_a_generation(path: &Path, spacing: Duration) -> io::Result<()> {
    // Asked before the tree is read: this runs on every persist and answers
    // "nothing to do" on almost all of them, so reading the whole document
    // first would be a full read per write for one copy every five minutes.
    let newest = backup_path(path, 0);
    let too_soon = std::fs::metadata(&newest)
        .and_then(|meta| meta.modified())
        .map(|at| at.elapsed().unwrap_or_default() < spacing)
        .unwrap_or(false);
    if too_soon {
        return Ok(());
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        // Nothing has been written yet, so there is no previous generation.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    // Oldest first, so nothing is overwritten before it has been moved down.
    // A generation that is not there yet simply has nothing to move.
    for generation in (1..BACKUP_GENERATIONS).rev() {
        let from = backup_path(path, generation - 1);
        match std::fs::rename(&from, backup_path(path, generation)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    crate::core::config::write_atomic_private(&newest, &bytes)
}

#[cfg(test)]
mod tests;
