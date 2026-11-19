//! The file tree's per-tab selection record.
//!
//! Owned by the workbench editor because [`crate::code_editor::EditorPanelState`]
//! retains one per tab; the file tree itself drives it through these methods.
//! Paths from a different `Host` never become selected.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lyrel_ui_base::host_ops::HostId;

/// Retained by one tab. Paths from a different Host never become selected.
#[derive(Default)]
pub struct TreeSelection {
    host: Option<HostId>,
    paths: HashSet<PathBuf>,
    cursor: Option<PathBuf>,
    anchor: Option<PathBuf>,
}

impl TreeSelection {
    pub fn cursor(&self, host: HostId) -> Option<&Path> {
        (self.host == Some(host))
            .then_some(self.cursor.as_deref())
            .flatten()
    }

    pub fn anchor(&self, host: HostId) -> Option<&Path> {
        (self.host == Some(host))
            .then_some(self.anchor.as_deref())
            .flatten()
    }

    pub fn contains(&self, host: HostId, path: &Path) -> bool {
        self.host == Some(host) && self.paths.contains(path)
    }

    pub fn single(&mut self, host: HostId, path: PathBuf) {
        self.host = Some(host);
        self.paths.clear();
        self.paths.insert(path.clone());
        self.cursor = Some(path.clone());
        self.anchor = Some(path);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Switch to `host`, dropping the previous selection when it changes.
    pub fn ensure_host(&mut self, host: HostId) {
        if self.host != Some(host) {
            self.clear();
            self.host = Some(host);
        }
    }

    /// Range-select the given paths and put the cursor on `cursor`.
    pub fn replace_range(&mut self, paths: Vec<PathBuf>, cursor: PathBuf) {
        self.paths.extend(paths);
        self.cursor = Some(cursor);
    }

    pub fn clear_paths(&mut self) {
        self.paths.clear();
    }

    /// Toggle one path; the cursor and anchor follow it either way.
    pub fn toggle_path(&mut self, path: &Path) {
        if !self.paths.remove(path) {
            self.paths.insert(path.to_path_buf());
        }
        self.cursor = Some(path.to_path_buf());
        self.anchor = Some(path.to_path_buf());
    }

    pub fn remove_tree(&mut self, host: HostId, path: &Path) {
        if self.host != Some(host) {
            return;
        }
        self.paths.retain(|p| !p.starts_with(path));
        if self.cursor.as_deref().is_some_and(|p| p.starts_with(path)) {
            self.cursor = self.paths.iter().min().cloned();
        }
        if self.anchor.as_deref().is_some_and(|p| p.starts_with(path)) {
            self.anchor = self.cursor.clone();
        }
    }

    pub fn targets(&self, host: HostId, roots: &[PathBuf], allow_roots: bool) -> Vec<PathBuf> {
        if self.host != Some(host) {
            return Vec::new();
        }
        let mut paths: Vec<_> = self
            .paths
            .iter()
            .filter(|p| allow_roots || !roots.contains(p))
            .cloned()
            .collect();
        paths.sort();
        let mut targets: Vec<PathBuf> = Vec::new();
        for path in paths {
            if !targets.iter().any(|parent| path.starts_with(parent)) {
                targets.push(path);
            }
        }
        targets
    }
}
