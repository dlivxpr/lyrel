//! CSV/TSV: the complete logical table, stable identities and local splices.
//!
//! The table holds source byte ranges only; field values are decoded from the
//! same snapshot on demand. `CsvDocumentState` is the revision/generation
//! guard around one document's table parse: a late result never replaces a
//! newer source, and a cancelled parse is retried once rather than published.

pub mod edit;
pub mod table;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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
/// workbench adapter.
#[derive(Default)]
pub struct CsvDocumentState {
    table: Option<Arc<CsvTable>>,
    error: Option<CsvTableError>,
    parsing: bool,
    generation: u64,
    parse_running: bool,
    cancel: Arc<AtomicBool>,
}

impl CsvDocumentState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The source changed: drop the old table (its ranges no longer describe
    /// the current revision), cancel any in-flight parse and request a fresh
    /// one. Returns true when the caller must schedule the debounced parse.
    pub fn invalidate(&mut self) -> bool {
        self.cancel.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.table = None;
        self.error = None;
        self.parsing = true;
        !self.parse_running
    }

    /// A debounced parse is about to read the snapshot. Returns the shared
    /// cancellation flag; `end_parse` must run when its result is handled.
    pub fn begin_parse(&mut self) -> Arc<AtomicBool> {
        self.parse_running = true;
        self.cancel.store(false, Ordering::Release);
        Arc::clone(&self.cancel)
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
        if !fresh {
            self.parsing = true;
            return TableInstall::Retry;
        }
        self.parsing = false;
        match outcome {
            Some(Ok(table)) => {
                self.error = None;
                self.table = Some(Arc::new(table));
                TableInstall::Installed
            }
            Some(Err(CsvTableError::Cancelled)) | None => {
                self.parsing = true;
                TableInstall::Retry
            }
            Some(Err(error)) => {
                self.table = None;
                self.error = Some(error);
                TableInstall::Installed
            }
        }
    }

    /// Cancels the parse on document drop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn table(&self) -> Option<Arc<CsvTable>> {
        self.table.clone()
    }

    pub fn error(&self) -> Option<CsvTableError> {
        self.error.clone()
    }

    pub fn is_parsing(&self) -> bool {
        self.parsing
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use table::DEFAULT_LIMITS;

    fn table(revision: u64) -> CsvTable {
        CsvTable::parse(
            Arc::new("a,b\n1,2\n".to_owned()),
            revision,
            b',',
            DEFAULT_LIMITS,
            &AtomicBool::new(false),
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
