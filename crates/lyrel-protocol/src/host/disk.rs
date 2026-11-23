//! Disk-usage wire values: one bounded directory scan and its containing volume.
//!
//! The scan itself — walking without following links, bounding the walk,
//! reading the volume table — lives in `lyrel-core`, which injects it into
//! the full local Host; this module owns only the values that cross the
//! wire, so a peer and its client read one schema. The answer is stamped by
//! the answering daemon exactly like
//! [`super::metrics::HostMetrics`]: a late reply from a replaced peer
//! instance is rejected by its client, never displayed.
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The sampler the full local Host is injected with: one bounded scan of a
/// directory it owns, run on the answering host. `None` reports `Unsupported`.
pub type DiskScanner = fn(&Path) -> io::Result<DiskUsage>;

/// One completed bounded scan of one directory on the owning host.
///
/// The walk never follows symbolic links or junctions, so a link inside the
/// tree counts as one entry — its target's contents are not visited, and a
/// link that points outside the tree cannot inflate the total. The walk is
/// bounded: when its entry budget is spent the scan stops and says so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirectoryScan {
    /// The exact directory that was scanned, as reported by the owning host.
    directory: String,
    /// Bytes counted for the files under `directory`. A hard link seen twice
    /// counts twice; a bounded walk prefers an honest, explainable total.
    size_bytes: u64,
    /// How many file entries contributed to `size_bytes`.
    files_counted: u64,
    /// Total capacity of the volume that owns `directory`; `None` when the
    /// platform cannot say, kept distinct from a partial traversal so the
    /// tile can mark an unknown total without implying the walk was cut
    /// short.
    volume_total_bytes: Option<u64>,
    /// `true` when the walk stopped at its bound or skipped unreadable
    /// entries: `size_bytes` is then a floor, not the whole tree.
    partial: bool,
    /// Representative entry-level errors, bounded; present when `partial`.
    errors: Vec<String>,
    /// The owning host's completion stamp.
    scanned_at_millis: u64,
}

impl DirectoryScan {
    /// One validated scan result from the owning host's walk.
    pub fn new(
        directory: impl Into<String>,
        size_bytes: u64,
        files_counted: u64,
        volume_total_bytes: Option<u64>,
        partial: bool,
        errors: Vec<String>,
        scanned_at_millis: u64,
    ) -> Self {
        Self {
            directory: directory.into(),
            size_bytes,
            files_counted,
            volume_total_bytes,
            partial,
            errors,
            scanned_at_millis,
        }
    }

    /// The exact directory that was scanned, on the host that scanned it.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// Bytes counted for the files under the scanned directory.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// How many file entries contributed to the size total.
    pub fn files_counted(&self) -> u64 {
        self.files_counted
    }

    /// Total capacity of the containing volume, or `None` when the platform
    /// cannot report it — distinct from [`Self::is_partial`], which says the
    /// traversal was cut short, not that the volume table was unreadable.
    pub fn volume_total_bytes(&self) -> Option<u64> {
        self.volume_total_bytes
    }

    /// `true` when the total is a floor: the bound was hit or entries errored.
    pub fn is_partial(&self) -> bool {
        self.partial
    }

    /// Representative entry-level errors behind a partial total.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The owning host's completion stamp.
    pub fn scanned_at_millis(&self) -> u64 {
        self.scanned_at_millis
    }
}

/// The answer to one directory-scan request: the requested directory, the
/// answering daemon's instance, and what the scan found. A peer that cannot
/// scan says [`DiskState::Unavailable`] with its reason; a directory the
/// host cannot read answers as an error instead, so a missing path never
/// masquerades as an empty one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiskUsage {
    directory: String,
    instance: String,
    state: DiskState,
}

impl DiskUsage {
    /// One stamped answer from the owning host's scan.
    pub fn new(
        directory: impl Into<String>,
        instance: impl Into<String>,
        state: DiskState,
    ) -> Self {
        Self {
            directory: directory.into(),
            instance: instance.into(),
            state,
        }
    }

    /// The directory this answer was requested for.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// The answering daemon's instance, matching the control hello, so a
    /// previous daemon's late answer cannot be reused.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// What the scan found, or why the platform could not provide it.
    pub fn state(&self) -> &DiskState {
        &self.state
    }

    /// Replace the state without changing the directory or instance stamp.
    pub fn set_state(&mut self, state: DiskState) {
        self.state = state;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskState {
    /// The bounded scan completed; the payload carries exactly what the
    /// walk saw, including whether the total is partial.
    Scanned(DirectoryScan),
    /// The host cannot scan disks right now; the reason says why.
    Unavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_result_round_trips_and_exposes_partial() {
        let scan = DirectoryScan::new(
            "/repo",
            4_600_000_000,
            42,
            Some(512_000_000_000),
            true,
            vec!["/repo/locked: permission denied".into()],
            1_700_000_000_000,
        );
        assert!(scan.is_partial());
        assert_eq!(scan.errors().len(), 1);
        assert_eq!(scan.directory(), "/repo");
        assert_eq!(scan.files_counted(), 42);
        assert_eq!(scan.volume_total_bytes(), Some(512_000_000_000));
    }

    #[test]
    fn unknown_capacity_is_not_partial() {
        // A missing volume total must not read as a cut-short walk: the two
        // states are displayed differently and mean different things.
        let scan = DirectoryScan::new("/repo", 10, 1, None, false, Vec::new(), 1_700_000_000_000);
        assert!(!scan.is_partial());
        assert_eq!(scan.volume_total_bytes(), None);
    }
}
