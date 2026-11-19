//! CSV/TSV: the complete logical table, stable identities and local splices.
//!
//! The table holds source byte ranges only; field values are decoded from the
//! same snapshot on demand. `CsvDocumentState` is the revision/generation
//! guard around one document's table parse — the same [`ProjectionState`]
//! interface the structured formats use, so a late result never replaces a
//! newer source, and a cancelled parse is retried once rather than published.

pub mod edit;
pub mod table;

pub use crate::state::CancelToken;
use crate::state::{ProjectionResult, ProjectionState};

use table::{CsvTable, CsvTableError};

/// What a finished table parse means to the caller's schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableInstall {
    /// A table or a visible failure is installed; no further parse is needed.
    Installed,
    /// The result was stale or cancelled; the current source still needs one.
    Retry,
}

/// One CSV/TSV document's table lifecycle. The shared text `Document`,
/// revision checks against it, debounce and off-thread parse stay in the
/// workbench adapter; the guard itself is [`ProjectionState`].
pub struct CsvDocumentState {
    state: ProjectionState<CsvTable, CsvTableError>,
    parse_running: bool,
}

impl Default for CsvDocumentState {
    fn default() -> Self {
        Self::new()
    }
}

impl CsvDocumentState {
    pub fn new() -> Self {
        Self {
            state: ProjectionState::new(),
            parse_running: false,
        }
    }

    /// The source changed: drop the old table (its ranges no longer describe
    /// the current revision), cancel any in-flight parse and request a fresh
    /// one. Returns true when the caller must schedule the debounced parse.
    pub fn invalidate(&mut self) -> bool {
        let start = !self.parse_running;
        self.state.invalidate();
        start
    }

    /// A debounced parse is about to read the snapshot. Returns the shared
    /// cancellation token; `end_parse` must run when its result is handled.
    pub fn begin_parse(&mut self) -> CancelToken {
        self.parse_running = true;
        self.state.begin_worker();
        self.state
            .take_job()
            .expect("a csv parse is pending after invalidate")
            .cancel()
            .clone()
    }

    /// The off-thread parse returned; its result is about to be installed.
    pub fn end_parse(&mut self) {
        self.parse_running = false;
    }

    /// Installs a result only when the caller has already verified document
    /// identity, revision and generation. Otherwise the old table stays
    /// discarded and a retry is requested.
    pub fn install(
        &mut self,
        fresh: bool,
        outcome: Option<Result<CsvTable, CsvTableError>>,
    ) -> TableInstall {
        self.parse_running = false;
        if !fresh {
            self.state.finish_job();
            self.state.rearm();
            return TableInstall::Retry;
        }
        let generation = self.state.generation();
        match outcome {
            Some(Ok(table)) => {
                self.state
                    .install(generation, 0, 0, ProjectionResult::Ready(table));
                TableInstall::Installed
            }
            Some(Err(CsvTableError::Cancelled)) | None => {
                self.state
                    .install(generation, 0, 0, ProjectionResult::Cancelled);
                self.state.rearm();
                TableInstall::Retry
            }
            Some(Err(error)) => {
                self.state
                    .install(generation, 0, 0, ProjectionResult::Failed(error));
                TableInstall::Installed
            }
        }
    }

    /// Cancels the parse on document drop.
    pub fn cancel(&self) {
        self.state.cancel();
    }

    pub fn table(&self) -> Option<std::sync::Arc<CsvTable>> {
        self.state.shared_projection()
    }

    pub fn error(&self) -> Option<CsvTableError> {
        self.state.failure()
    }

    pub fn is_parsing(&self) -> bool {
        self.state.is_parsing()
    }

    pub fn generation(&self) -> u64 {
        self.state.generation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use table::DEFAULT_LIMITS;

    fn table(revision: u64) -> CsvTable {
        CsvTable::parse(
            Arc::new("a,b\n1,2\n".to_owned()),
            revision,
            b',',
            DEFAULT_LIMITS,
            &CancelToken::new(),
        )
        .expect("fixture table")
    }

    #[test]
    fn a_stale_result_never_replaces_the_table_and_asks_for_a_retry() {
        let mut state = CsvDocumentState::new();
        assert!(state.invalidate());
        state.begin_parse();
        state.end_parse();
        assert_eq!(
            state.install(false, Some(Ok(table(1)))),
            TableInstall::Retry
        );
        assert!(state.table().is_none());
        assert!(state.is_parsing());

        state.begin_parse();
        state.end_parse();
        assert_eq!(
            state.install(true, Some(Ok(table(2)))),
            TableInstall::Installed
        );
        assert_eq!(state.table().map(|table| table.revision()), Some(2));
        assert!(!state.is_parsing());
    }

    #[test]
    fn a_cancelled_or_missing_parse_is_retried_without_clearing_the_error_slot() {
        let mut state = CsvDocumentState::new();
        state.invalidate();
        state.begin_parse();
        state.end_parse();
        assert_eq!(state.install(true, None), TableInstall::Retry);
        assert!(state.is_parsing());

        state.begin_parse();
        state.end_parse();
        assert_eq!(
            state.install(true, Some(Err(CsvTableError::Cancelled))),
            TableInstall::Retry
        );
        assert_eq!(state.error(), None);
        assert!(state.is_parsing());
    }

    #[test]
    fn a_visible_failure_is_installed_without_a_retry_and_a_new_edit_clears_it() {
        let mut state = CsvDocumentState::new();
        state.invalidate();
        state.begin_parse();
        state.end_parse();
        assert_eq!(
            state.install(true, Some(Err(CsvTableError::Reader("reader".to_owned())))),
            TableInstall::Installed
        );
        assert_eq!(
            state.error(),
            Some(CsvTableError::Reader("reader".to_owned()))
        );
        assert!(!state.is_parsing());
        assert!(state.invalidate());
        assert_eq!(state.error(), None);
        assert!(state.table().is_none());
    }

    #[test]
    fn invalidate_reuses_a_running_parse_and_advances_the_generation() {
        let mut state = CsvDocumentState::new();
        assert!(state.invalidate());
        assert_eq!(state.generation(), 1);
        state.begin_parse();
        assert!(!state.invalidate());
        assert_eq!(state.generation(), 2);
        state.end_parse();
        assert!(state.invalidate(), "no parse is running after end_parse");
    }
}
