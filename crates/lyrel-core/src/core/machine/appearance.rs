//! GUI-written appearance hint, separate from the daemon-owned tree.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(test)]
use super::paths::APPEARANCE_FILE;
use super::paths::appearance_path;

/// The light/dark mode the GUI last applied, cached beside the machine tree.
///
/// The daemon needs it when it spawns a pane on Windows, to fill in the
/// `COLORFGBG` hint (see `daemon::pane::pane_environment`), and the daemon is a
/// separate process from the GUI that owns the theme. This is derived state,
/// not a setting: it is
/// written by whichever process paints the window and read by whoever needs to
/// describe that window, so it lives here rather than in `config.json` — the
/// user's file, which nothing should rewrite behind their back.
///
/// A file of its own rather than a field on [`super::Machine`]: the machine tree is
/// owned by the daemon, held in memory and flushed on a timer, so a second
/// writer would clobber the workspaces and panes it had not seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub dark: bool,
}

/// Read the cached appearance, or the default when there is none to read.
///
/// Absent, unreadable, and unparsable all answer `dark: false`, which is what
/// lyrel's default preset is: a daemon that spawns a pane before the GUI has
/// ever applied a theme describes the default window rather than guessing.
pub fn appearance() -> Appearance {
    match appearance_path() {
        Ok(path) => read_appearance(&path),
        Err(e) => {
            log::debug!("no appearance hint ({e}); assuming the default preset");
            Appearance::default()
        }
    }
}

/// Record the appearance the GUI just applied. A no-op when it has not changed,
/// so repainting the same theme does not touch the disk.
pub fn note_appearance(dark: bool) {
    let Ok(path) = appearance_path() else { return };
    let next = Appearance { dark };
    if read_appearance(&path) == next && path.exists() {
        return;
    }
    if let Err(e) = write_appearance(&path, next) {
        log::warn!("could not write {}: {e}", path.display());
    }
}

fn read_appearance(path: &Path) -> Appearance {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Appearance::default();
    };
    serde_json::from_str::<Appearance>(crate::core::config::strip_bom(&text)).unwrap_or_else(|e| {
        log::warn!(
            "{} does not parse ({e}); assuming the default preset",
            path.display()
        );
        Appearance::default()
    })
}

fn write_appearance(path: &Path, appearance: Appearance) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&appearance).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::core::config::write_atomic_private(path, &bytes)
}

#[cfg(test)]
mod tests;
