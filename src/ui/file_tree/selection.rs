use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{HostId, TreeRow};

/// Retained by one tab. Paths from a different Host never become selected.
#[derive(Default)]
pub(crate) struct TreeSelection {
    host: Option<HostId>,
    paths: HashSet<PathBuf>,
    cursor: Option<PathBuf>,
    anchor: Option<PathBuf>,
}

impl TreeSelection {
    pub(super) fn cursor(&self, host: HostId) -> Option<&Path> {
        (self.host == Some(host))
            .then_some(self.cursor.as_deref())
            .flatten()
    }

    pub(super) fn contains(&self, host: HostId, path: &Path) -> bool {
        self.host == Some(host) && self.paths.contains(path)
    }

    pub(super) fn single(&mut self, host: HostId, path: PathBuf) {
        self.host = Some(host);
        self.paths.clear();
        self.paths.insert(path.clone());
        self.anchor = Some(path.clone());
        self.cursor = Some(path);
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn remove_tree(&mut self, host: HostId, path: &Path) {
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

    pub(super) fn click(
        &mut self,
        host: HostId,
        path: &Path,
        toggle: bool,
        range: bool,
        rows: &[TreeRow],
    ) {
        if self.host != Some(host) {
            self.clear();
            self.host = Some(host);
        }
        if range {
            let anchor = self.anchor.as_deref().or(self.cursor.as_deref());
            if let Some((a, b)) = anchor.and_then(|anchor| {
                Some((
                    rows.iter().position(|r| r.entry.path == anchor)?,
                    rows.iter().position(|r| r.entry.path == path)?,
                ))
            }) {
                if !toggle {
                    self.paths.clear();
                }
                self.paths.extend(
                    rows[a.min(b)..=a.max(b)]
                        .iter()
                        .filter(|r| r.note.is_none())
                        .map(|r| r.entry.path.clone()),
                );
                self.cursor = Some(path.to_path_buf());
                return;
            }
        }
        if toggle {
            if !self.paths.remove(path) {
                self.paths.insert(path.to_path_buf());
            }
            self.cursor = Some(path.to_path_buf());
            self.anchor = Some(path.to_path_buf());
        } else {
            self.single(host, path.to_path_buf());
        }
    }

    pub(super) fn targets(
        &self,
        host: HostId,
        roots: &[PathBuf],
        allow_roots: bool,
    ) -> Vec<PathBuf> {
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

pub(super) fn primary_modifier(modifiers: gpui_kit::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}
