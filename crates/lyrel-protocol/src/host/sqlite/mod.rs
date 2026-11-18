//! Read-only SQLite browsing types: the wire values, the request/reply enums
//! and the operation-scoped cancellation token.
//!
//! The read-only engine behind these types — the `rusqlite` connection pool,
//! per-view transaction and sweeper — stays in `lyrel-core`; it implements
//! [`SqliteEngine`] and a full local Host gets it injected at construction.
//! The C-free base `LocalHost` carries no engine and reports `Unsupported`,
//! exactly like a transport that cannot browse at all.
//!
//! Ported from Glim `crates/glim-services/src/sqlite/browser.rs` (MIT,
//! `fc2183f2c5f27ddfcdc4badbab6d83d50616831b`): identifier quoting, the
//! single-parameter substring predicate and its parenthesis tree, the
//! deterministic row order, the bounded column window, page rows and the
//! value chunk. Glim's [`crate::host::transfer`] cancellation chain is reused
//! for the one request that asks for a page.
//!
//! Replaced, not imported: Glim's `sqlite.rs::Snapshot` copies `main`, `-wal`
//! and `-journal` into a temp directory and reopens them read-write for
//! recovery. Lyrel opens the source read-only once per read view and keeps one
//! SQLite transaction for the metadata and every page of that view. A page is
//! never stitched from a different database version, and the wire carries only
//! schema metadata, bounded rows and value chunks — never the file.
//!
//! A read view is bounded twice: by idleness and by absolute age. Both are
//! reported in [`SqliteOpened`] so a client can say what it is looking at; an
//! expired view is reported as [`StaleView`], never silently replaced by a
//! newer snapshot halfway through a page. Refreshing opens a new view with a
//! new identity.

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Rows in one page. Glim's `reader-limits.json` value.
pub const PAGE_ROWS: u64 = 500;
/// Columns in one page request. Wider tables are paged horizontally.
pub const COLUMN_WINDOW: u32 = 32;
/// Bytes of a TEXT value kept for a row preview.
pub const TEXT_PREVIEW_BYTES: u64 = 512;
/// Bytes of a BLOB value kept for a row preview.
pub const BLOB_PREVIEW_BYTES: u64 = 16;
/// Bytes in one full-value chunk.
pub const VALUE_CHUNK_BYTES: u64 = 1024 * 1024;
/// Default SQLite query budget, in seconds.
pub const QUERY_SECONDS: u64 = 5;
/// Ceiling for a query budget a client may ask for.
pub const QUERY_SECONDS_MAX: u64 = 60;
/// How long to wait on a busy database before reporting it as busy.
pub const BUSY_MILLIS: u64 = 1_000;
/// Page cache per connection.
pub const CACHE_KIB: u64 = 8 * 1024;
/// A read view no request has touched for this long is retired.
pub const VIEW_IDLE: Duration = Duration::from_secs(30);
/// A read view older than this is retired even while it is being used.
pub const VIEW_MAX_AGE: Duration = Duration::from_secs(180);
/// Read views one process-wide service keeps open at once.
pub const MAX_VIEWS: usize = 64;

/// The read-only SQLite browsing engine a full local Host carries.
///
/// One engine per Host, so a remote control connection tracks and closes only
/// its own view ids. Implementations own the read-only connection pool and
/// the bounded view lifetime; nothing here writes, recovers or changes the
/// journal mode.
pub trait SqliteEngine: Send + Sync {
    /// Open `path` read-only and fix the read view its schema and pages share.
    fn open(&self, path: &Path, cancel: &SqliteCancel) -> io::Result<SqliteOpened>;

    /// The columns of one table or view in an open read view's version.
    fn columns(
        &self,
        view: u64,
        table: &str,
        cancel: &SqliteCancel,
    ) -> io::Result<SqliteColumns>;

    fn count(&self, view: u64, query: &SqliteQuery, cancel: &SqliteCancel)
    -> io::Result<u64>;

    fn page(
        &self,
        view: u64,
        query: &SqliteQuery,
        cancel: &SqliteCancel,
    ) -> io::Result<SqlitePage>;

    /// One complete cell value, in a bounded chunk. TEXT and BLOB return raw
    /// bytes from `offset`; INTEGER and REAL come back whole and exact.
    fn value(
        &self,
        view: u64,
        query: &SqliteQuery,
        row: u32,
        column: u32,
        offset: u64,
        max_bytes: u64,
        cancel: &SqliteCancel,
    ) -> io::Result<SqliteValue>;

    /// Close a read view early. Unknown ids are already closed; this must not
    /// block the caller and must never be a write.
    fn close(&self, view: u64);
}

/// The objects a SQLite schema can expose for browsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteObjectKind {
    Table,
    View,
}

/// One table or view, with the SQL definition SQLite stores for it.
///
/// `sql` is empty for an implicit object such as `sqlite_sequence`; it is the
/// definition as stored, not a reconstruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteObject {
    pub name: String,
    pub kind: SqliteObjectKind,
    pub sql: String,
}

/// One opened read view and the schema it sees.
///
/// `view` is opaque and scoped to the Host service that opened it. The schema
/// was read inside the same transaction as the pages that follow, so a table
/// listed here always has that definition in this view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteOpened {
    pub view: u64,
    pub objects: Vec<SqliteObject>,
    /// `delete`, `wal`, or another journal mode as SQLite spells it.
    pub journal_mode: String,
    /// The bounded lifetime behind this view, so a client does not guess.
    pub idle_millis: u64,
    pub max_age_millis: u64,
}

/// The columns of one table, and the deterministic order used to page it.
///
/// `order` is the rowid alias where the table has one, otherwise its primary
/// key columns in key order, otherwise every column. It is the stable
/// tie-breaker a page query must send back with its own sort.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteColumns {
    pub names: Vec<String>,
    pub order: Vec<String>,
}

/// What one page asks for.
///
/// Every identifier here is checked against the view's own schema and quoted
/// by the engine; the search text stays a bound parameter. There is no place
/// for arbitrary SQL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteQuery {
    pub table: String,
    pub columns: Vec<String>,
    pub order: Vec<String>,
    /// One substring matched case-insensitively against every listed column.
    pub search: String,
    /// `(index into `columns`, descending)`.
    pub sort: Option<(u32, bool)>,
    pub page: u64,
    pub start_column: u32,
    pub seconds: u64,
}

/// One value with its SQLite storage class intact.
///
/// `Real` carries the IEEE-754 bit pattern: JSON has no NaN or infinity, and
/// a display string would not round-trip every `f64`. `Text` and `Blob` keep
/// raw bytes, not a lossy UTF-8 string; `total` is the full byte length, so a
/// caller can tell a complete value from a bounded preview.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SqliteValue {
    Null,
    Integer {
        value: i64,
    },
    Real {
        bits: u64,
    },
    Text {
        #[serde(with = "crate::host::b64")]
        preview: Vec<u8>,
        total: u64,
    },
    Blob {
        #[serde(with = "crate::host::b64")]
        preview: Vec<u8>,
        total: u64,
    },
}

impl SqliteValue {
    /// Whether this preview is shorter than the value it came from.
    pub fn is_truncated(&self) -> bool {
        match self {
            Self::Text { preview, total } | Self::Blob { preview, total } => {
                (preview.len() as u64) < *total
            }
            _ => false,
        }
    }

    pub fn real(bits: u64) -> Self {
        Self::Real { bits }
    }

    /// The exact `f64` a `Real` carries.
    pub fn real_value(&self) -> Option<f64> {
        match self {
            Self::Real { bits } => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

/// One page of rows, with the identity that produced it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlitePage {
    /// The page number actually read, after the request was clamped.
    pub page: u64,
    /// The first column actually read, after the request was clamped.
    pub start_column: u32,
    pub rows: Vec<Vec<SqliteValue>>,
    /// Whether the table has at least one more row after this page.
    pub more: bool,
}

/// One request on a read view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SqliteRequest {
    /// Open `path` read-only and fix one read view; never writes or recovers.
    Open {
        path: String,
    },
    /// Close a view early. Also used for refresh's old view.
    Close {
        view: u64,
    },
    Columns {
        view: u64,
        table: String,
    },
    Count {
        view: u64,
        query: SqliteQuery,
    },
    Page {
        view: u64,
        query: SqliteQuery,
    },
    /// The complete value of one cell, in chunks of at most
    /// [`VALUE_CHUNK_BYTES`]. `offset` applies to TEXT and BLOB.
    Value {
        view: u64,
        query: SqliteQuery,
        row: u32,
        column: u32,
        offset: u64,
        max_bytes: u64,
    },
}

/// One answer to a [`SqliteRequest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteReply {
    Opened(SqliteOpened),
    Closed,
    Columns(SqliteColumns),
    Count(u64),
    Page(SqlitePage),
    Value(SqliteValue),
}

/// A read view that was closed, replaced by a refresh, retired for idleness
/// or age, or lost with its connection.
///
/// This is its own error so a client can say "this view expired; refresh reads
/// a new one" instead of presenting a stale page as the current database.
#[derive(Debug)]
pub struct StaleView {
    pub message: String,
}

impl fmt::Display for StaleView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StaleView {}

pub fn stale_view(view: u64) -> io::Error {
    stale_view_message(format!(
        "read view {view} is no longer open; refresh to read a new one"
    ))
}

/// Rebuild the [`StaleView`] marker after it crossed the wire.
pub fn stale_view_message(message: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        StaleView {
            message: message.into(),
        },
    )
}

pub fn is_stale_view(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<StaleView>().is_some())
}

struct CancelInner {
    cancelled: AtomicBool,
    next_hook: AtomicU64,
    hooks: Mutex<Vec<(u64, Box<dyn FnOnce() + Send + 'static>)>>,
}

/// Cancellation bound to one operation, not to a read view.
///
/// A page query keeps its own token: cancelling the column request that came
/// before it, or the page it replaced, cannot reach this one. A remote caller
/// registers a hook that posts the wire `CANCEL` for exactly the request in
/// flight; the Host side registers the connection's interrupt handle, so a
/// `sqlite3_interrupt` stops the statement even between progress callbacks.
#[derive(Clone, Default)]
pub struct SqliteCancel {
    inner: Arc<CancelInner>,
}

impl Default for CancelInner {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            next_hook: AtomicU64::new(1),
            hooks: Mutex::new(Vec::new()),
        }
    }
}

impl SqliteCancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trip the token and run every registered hook once.
    pub fn cancel(&self) {
        if self.inner.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let hooks = {
            let mut hooks = self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *hooks)
        };
        for (_, hook) in hooks {
            hook();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            // The same wording the rest of the Host uses for a cancelled
            // transfer, so callers can treat them alike.
            return Err(crate::host::transfer::Cancelled.into_io());
        }
        Ok(())
    }

    /// Run `hook` when the token trips, or immediately when it already has.
    ///
    /// The returned guard unregisters a hook that has not fired; a consumer
    /// drops it when its operation ends, so a later cancellation cannot post a
    /// `CANCEL` for a request the peer already answered.
    pub fn on_cancel<F: FnOnce() + Send + 'static>(&self, hook: F) -> CancelHook {
        let id = self.inner.next_hook.fetch_add(1, Ordering::Relaxed);
        if self.is_cancelled() {
            hook();
            return CancelHook {
                inner: Arc::clone(&self.inner),
                id: None,
            };
        }
        let mut hooks = self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner());
        // Re-check under the lock: `cancel` marks first and drains after, so
        // a hook installed after the mark would otherwise be lost.
        if self.is_cancelled() {
            drop(hooks);
            hook();
            return CancelHook {
                inner: Arc::clone(&self.inner),
                id: None,
            };
        }
        hooks.push((id, Box::new(hook)));
        CancelHook {
            inner: Arc::clone(&self.inner),
            id: Some(id),
        }
    }

    #[cfg(test)]
    pub(crate) fn hook_count(&self) -> usize {
        self.inner
            .hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

impl fmt::Debug for SqliteCancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteCancel")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Removes one cancellation hook when dropped.
#[must_use = "dropping this guard unregisters the hook"]
pub struct CancelHook {
    inner: Arc<CancelInner>,
    id: Option<u64>,
}

impl Drop for CancelHook {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        self.inner
            .hooks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(hook_id, _)| *hook_id != id);
    }
}
