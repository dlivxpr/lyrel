//! One read view per open database: a read-only connection and one deferred
//! transaction, bounded by idleness and absolute age.
//!
//! The transaction is started before the schema is read, so the table list,
//! each column list and every page of this view come from the same database
//! version. Opening is never a write open: no recovery, no journal-mode change
//! and no extension loading happens here. A view that SQLite cannot open
//! read-only is reported, not repaired.
//!
//! Views are retired by a process-wide sweeper, by the client's close, or by
//! the connection that owns them. Retirement interrupts whatever statement is
//! in flight and closes the connection as soon as the operation holding it
//! returns, which rolls the read transaction back and lets WAL checkpointing
//! proceed again.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use rusqlite::{
    Connection, OpenFlags,
    config::DbConfig,
    hooks::{AuthAction, AuthContext, Authorization, TransactionOperation},
};

use super::{
    MAX_VIEWS, QUERY_SECONDS, QUERY_SECONDS_MAX, SqliteCancel, SqliteColumns, SqliteEngine,
    SqliteObject, SqliteOpened, SqlitePage, SqliteQuery, SqliteValue, VALUE_CHUNK_BYTES, VIEW_IDLE,
    VIEW_MAX_AGE, browser, stale_view,
};

/// Lifetime knobs for read views. Production uses [`SqliteLimits::default`];
/// tests shorten them instead of sleeping for minutes.
#[derive(Clone, Copy, Debug)]
pub struct SqliteLimits {
    pub idle: Duration,
    pub max_age: Duration,
}

impl Default for SqliteLimits {
    fn default() -> Self {
        Self {
            idle: VIEW_IDLE,
            max_age: VIEW_MAX_AGE,
        }
    }
}

struct ViewHandle {
    connection: Mutex<Connection>,
    interrupt: Arc<rusqlite::InterruptHandle>,
    created: Instant,
    last_used: Mutex<Instant>,
    retired: AtomicBool,
    objects: Vec<SqliteObject>,
    columns: Mutex<HashMap<String, SqliteColumns>>,
}

impl ViewHandle {
    fn expired(&self, now: Instant, limits: SqliteLimits) -> bool {
        now.duration_since(self.created) >= limits.max_age
            || now.duration_since(*self.last_used.lock().unwrap_or_else(|e| e.into_inner()))
                >= limits.idle
    }
}

struct Inner {
    limits: SqliteLimits,
    views: Mutex<HashMap<u64, Arc<ViewHandle>>>,
    next: AtomicU64,
}

/// The SQLite side of one Host.
///
/// `LocalHost` owns one; a control connection does not need its own because it
/// opens, tracks and closes views through the Host it serves.
pub struct SqliteService {
    inner: Arc<Inner>,
}

impl Clone for SqliteService {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for SqliteService {
    fn default() -> Self {
        Self::new()
    }
}

impl SqliteService {
    pub fn new() -> Self {
        Self::with_limits(SqliteLimits::default())
    }

    pub fn with_limits(limits: SqliteLimits) -> Self {
        let inner = Arc::new(Inner {
            limits,
            views: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        });
        reaper().register(&inner);
        Self { inner }
    }

    /// Open `path` read-only and fix the read view its schema and pages share.
    pub fn open(&self, path: &Path, cancel: &SqliteCancel) -> io::Result<SqliteOpened> {
        cancel.check()?;
        let metadata = std::fs::metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} no longer exists", path.display()),
                )
            } else {
                error
            }
        })?;
        if metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!("{} is a directory", path.display()),
            ));
        }
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", path.display()),
            ));
        }

        let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| open_error(path, error))?;
        connection
            .busy_timeout(Duration::from_millis(super::BUSY_MILLIS))
            .map_err(|error| map_error(error))?;
        for (config, value) in [
            (DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
            (DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true),
            (DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
        ] {
            if let Err(error) = connection.set_db_config(config, value) {
                // An older bundled SQLite may not know one of these; the
                // read-only flag and the authorizer still apply.
                log::debug!("sqlite: {config:?} was not applied: {error}");
            }
        }
        connection
            .execute_batch(&format!(
                "PRAGMA query_only=ON; PRAGMA cache_size=-{};",
                super::CACHE_KIB
            ))
            .map_err(|error| open_error(path, error))?;
        // Read-only by construction from here on: the request shapes never
        // carry DML, and this refuses the classes that could write or attach
        // another database if a malicious schema asked for them.
        connection
            .authorizer(Some(read_only_authorizer))
            .map_err(map_error)?;

        let interrupt = Arc::new(connection.get_interrupt_handle());
        let hook = cancel.on_cancel({
            let interrupt = Arc::clone(&interrupt);
            move || interrupt.interrupt()
        });
        set_budget(&mut connection, cancel, QUERY_SECONDS);
        cancel.check()?;
        // The transaction comes before the schema read: the table list, column
        // metadata and later pages all belong to one database version.
        connection
            .execute_batch("BEGIN DEFERRED")
            .map_err(|error| open_error(path, error))?;
        let opened = browser::opening(&connection).map_err(|error| open_error(path, error));
        clear_budget(&mut connection);
        drop(hook);
        let (journal_mode, objects) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        };
        cancel.check()?;

        let view = Arc::new(ViewHandle {
            connection: Mutex::new(connection),
            interrupt,
            created: Instant::now(),
            last_used: Mutex::new(Instant::now()),
            retired: AtomicBool::new(false),
            objects: objects.clone(),
            columns: Mutex::new(HashMap::new()),
        });
        self.inner.sweep();
        let id = {
            let mut views = self.inner.views.lock().unwrap_or_else(|e| e.into_inner());
            if views.len() >= MAX_VIEWS {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "this host already holds {MAX_VIEWS} SQLite read views; close one before opening another"
                    ),
                ));
            }
            let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
            views.insert(id, view);
            id
        };
        Ok(SqliteOpened {
            view: id,
            objects,
            journal_mode,
            idle_millis: millis(self.inner.limits.idle),
            max_age_millis: millis(self.inner.limits.max_age),
        })
    }

    /// The columns of one object in this view's schema version.
    pub fn columns(
        &self,
        view: u64,
        table: &str,
        cancel: &SqliteCancel,
    ) -> io::Result<SqliteColumns> {
        let handle = self.live_view(view, cancel)?;
        if let Some(columns) = handle
            .columns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(table)
            .cloned()
        {
            return Ok(columns);
        }
        if !handle.objects.iter().any(|object| object.name == table) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{table:?} is not a table or view in this read view"),
            ));
        }
        let operation = self.operation(view, &handle, cancel, QUERY_SECONDS)?;
        let columns = browser::columns(&operation.connection, table).map_err(map_error)?;
        drop(operation);
        handle
            .columns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(table.to_string(), columns.clone());
        Ok(columns)
    }

    pub fn count(&self, view: u64, query: &SqliteQuery, cancel: &SqliteCancel) -> io::Result<u64> {
        let columns = self.columns(view, &query.table, cancel)?;
        browser::validate(query, &columns).map_err(map_error)?;
        let handle = self.live_view(view, cancel)?;
        let operation = self.operation(view, &handle, cancel, seconds(query))?;
        let total = browser::count(&operation.connection, query).map_err(map_error)?;
        Ok(total)
    }

    pub fn page(
        &self,
        view: u64,
        query: &SqliteQuery,
        cancel: &SqliteCancel,
    ) -> io::Result<SqlitePage> {
        let columns = self.columns(view, &query.table, cancel)?;
        browser::validate(query, &columns).map_err(map_error)?;
        let handle = self.live_view(view, cancel)?;
        let operation = self.operation(view, &handle, cancel, seconds(query))?;
        let page = browser::page(&operation.connection, query).map_err(map_error)?;
        Ok(page)
    }

    pub fn value(
        &self,
        view: u64,
        query: &SqliteQuery,
        row: u32,
        column: u32,
        offset: u64,
        max_bytes: u64,
        cancel: &SqliteCancel,
    ) -> io::Result<SqliteValue> {
        if max_bytes == 0 || max_bytes > VALUE_CHUNK_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a value chunk must be 1..={VALUE_CHUNK_BYTES} bytes, not {max_bytes}"),
            ));
        }
        if row as u64 >= super::PAGE_ROWS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("row {row} is outside a {} row page", super::PAGE_ROWS),
            ));
        }
        let columns = self.columns(view, &query.table, cancel)?;
        browser::validate(query, &columns).map_err(map_error)?;
        let handle = self.live_view(view, cancel)?;
        let operation = self.operation(view, &handle, cancel, seconds(query))?;
        let value = browser::value(&operation.connection, query, row, column, offset, max_bytes)
            .map_err(map_error)?;
        Ok(value)
    }

    /// Close a view early. Unknown ids are already closed, so a duplicate
    /// close (an older tab, a retried request) is not an error.
    pub fn close(&self, view: u64) -> bool {
        let removed = {
            let mut views = self.inner.views.lock().unwrap_or_else(|e| e.into_inner());
            views.remove(&view)
        };
        let Some(handle) = removed else {
            return false;
        };
        retire(&handle);
        true
    }

    /// Retire every view. Used when the owning connection ends.
    pub fn clear(&self) {
        let handles = {
            let mut views = self.inner.views.lock().unwrap_or_else(|e| e.into_inner());
            views.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };
        for handle in handles {
            retire(&handle);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .views
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Run one sweep now. The background reaper uses the same path; tests call
    /// it directly so a short limit does not need a sleep race.
    #[cfg(test)]
    pub(crate) fn sweep_now(&self) {
        self.inner.sweep();
    }

    fn live_view(&self, view: u64, cancel: &SqliteCancel) -> io::Result<Arc<ViewHandle>> {
        cancel.check()?;
        let handle = {
            let views = self.inner.views.lock().unwrap_or_else(|e| e.into_inner());
            views.get(&view).cloned()
        };
        let Some(handle) = handle else {
            return Err(stale_view(view));
        };
        let now = Instant::now();
        if handle.retired.load(Ordering::Acquire)
            || (handle.expired(now, self.inner.limits) && self.inner.retire_if_idle(view, &handle))
        {
            return Err(stale_view(view));
        }
        *handle.last_used.lock().unwrap_or_else(|e| e.into_inner()) = now;
        Ok(handle)
    }

    fn operation<'a>(
        &self,
        view: u64,
        handle: &'a ViewHandle,
        cancel: &SqliteCancel,
        seconds: u64,
    ) -> io::Result<ViewOperation<'a>> {
        cancel.check()?;
        let mut connection = handle.connection.lock().unwrap_or_else(|e| e.into_inner());
        if handle.retired.load(Ordering::Acquire) {
            return Err(stale_view(view));
        }
        cancel.check()?;
        set_budget(&mut connection, cancel, seconds);
        let hook = cancel.on_cancel({
            let interrupt = Arc::clone(&handle.interrupt);
            move || interrupt.interrupt()
        });
        Ok(ViewOperation {
            connection,
            _hook: hook,
            last_used: &handle.last_used,
        })
    }
}

impl Inner {
    fn sweep(&self) {
        let now = Instant::now();
        let candidates = {
            let views = self.views.lock().unwrap_or_else(|e| e.into_inner());
            views
                .iter()
                .filter(|(_, handle)| handle.expired(now, self.limits))
                .map(|(id, handle)| (*id, Arc::clone(handle)))
                .collect::<Vec<_>>()
        };
        for (id, handle) in candidates {
            self.retire_if_idle(id, &handle);
        }
    }

    /// Remove `handle` if it is still the view registered under `id` and no
    /// operation is using its connection. Returns whether it was retired.
    fn retire_if_idle(&self, id: u64, handle: &Arc<ViewHandle>) -> bool {
        let Ok(connection) = handle.connection.try_lock() else {
            // An operation owns the connection; the next sweep retires it
            // after that operation returns, and the absolute age still bounds
            // how long it can run through its own query budget.
            return false;
        };
        let removed = {
            let mut views = self.views.lock().unwrap_or_else(|e| e.into_inner());
            let same = views
                .get(&id)
                .is_some_and(|registered| Arc::ptr_eq(registered, handle));
            if same {
                views.remove(&id);
            }
            same
        };
        if !removed {
            return false;
        }
        handle.retired.store(true, Ordering::Release);
        handle.interrupt.interrupt();
        let _ = connection.execute_batch("ROLLBACK");
        true
    }
}

fn retire(handle: &Arc<ViewHandle>) {
    handle.retired.store(true, Ordering::Release);
    handle.interrupt.interrupt();
    if let Ok(connection) = handle.connection.try_lock() {
        let _ = connection.execute_batch("ROLLBACK");
    }
}

/// One query on a view: the connection is held for the duration, the progress
/// handler carries the query budget, and the interrupt hook stops the
/// statement when the operation's own token trips.
struct ViewOperation<'a> {
    connection: MutexGuard<'a, Connection>,
    /// Dropped after the connection guard: a cancellation after this point
    /// cannot interrupt a statement this operation no longer owns.
    _hook: super::CancelHook,
    last_used: &'a Mutex<Instant>,
}

impl Drop for ViewOperation<'_> {
    fn drop(&mut self) {
        clear_budget(&mut self.connection);
        // Any request counts as use, including one that failed: the client
        // asked about this view, so it is not idle.
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}

fn seconds(query: &SqliteQuery) -> u64 {
    query.seconds.max(QUERY_SECONDS).min(QUERY_SECONDS_MAX)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn set_budget(connection: &mut Connection, cancel: &SqliteCancel, seconds: u64) {
    let token = cancel.clone();
    let deadline = Instant::now() + Duration::from_secs(seconds.max(1));
    let _ = connection.progress_handler(
        1_000,
        Some(move || token.is_cancelled() || Instant::now() >= deadline),
    );
}

fn clear_budget(connection: &mut Connection) {
    let _ = connection.progress_handler(0, None::<fn() -> bool>);
}

/// The only authorization policy a browser connection needs.
///
/// The query shapes never contain writes, but a schema can: a view definition
/// is executed by SQLite when it is read. Refusing write, attach, detach and
/// schema-change classes here means even a hostile view cannot make browsing
/// write, attach another file, or change the journal mode. Reads, SELECTs,
/// built-in functions and read-only pragmas stay available; extension loading
/// is not compiled into this build. Pragma arguments are allowed only for
/// the read-only table_xinfo metadata operation.
fn read_only_authorizer(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read { .. }
        | AuthAction::Select
        | AuthAction::Function { .. }
        | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Pragma {
            pragma_value: None, ..
        } => Authorization::Allow,
        // The sole argument-taking pragma in browser query shapes is a
        // metadata read, not a setter. Never permit arbitrary pragma values.
        AuthAction::Pragma {
            pragma_name: "table_xinfo",
            ..
        } => Authorization::Allow,
        AuthAction::Transaction {
            operation:
                TransactionOperation::Begin
                | TransactionOperation::Release
                | TransactionOperation::Rollback,
        } => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

fn open_error(path: &Path, error: rusqlite::Error) -> io::Error {
    let requires_recovery = matches!(
        &error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::CannotOpen | rusqlite::ErrorCode::DatabaseBusy
            )
    ) && has_sidecar(path, "-wal");
    if requires_recovery {
        return io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "could not open {} read-only; it may need WAL recovery, which browsing never performs: {error}",
                path.display()
            ),
        );
    }
    map_error(error)
}

fn has_sidecar(path: &Path, suffix: &str) -> bool {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::fs::metadata(PathBuf::from(name)).is_ok()
}

fn map_error(error: rusqlite::Error) -> io::Error {
    use rusqlite::Error as E;
    use rusqlite::ErrorCode;
    match error {
        E::SqliteFailure(failure, message) => {
            let detail = message.unwrap_or_else(|| failure.to_string());
            match failure.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("the database is busy; another connection holds its lock: {detail}"),
                ),
                ErrorCode::OperationInterrupted | ErrorCode::OperationAborted => io::Error::new(
                    io::ErrorKind::Interrupted,
                    format!("the SQLite query was interrupted: {detail}"),
                ),
                ErrorCode::DatabaseCorrupt => io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the database file is corrupt: {detail}"),
                ),
                ErrorCode::NotADatabase => io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the file is not a SQLite database: {detail}"),
                ),
                ErrorCode::PermissionDenied | ErrorCode::ReadOnly => {
                    io::Error::new(io::ErrorKind::PermissionDenied, detail)
                }
                ErrorCode::CannotOpen => io::Error::other(detail),
                ErrorCode::ApiMisuse => io::Error::new(io::ErrorKind::InvalidInput, detail),
                _ => io::Error::other(detail),
            }
        }
        E::QueryReturnedNoRows => io::Error::new(
            io::ErrorKind::NotFound,
            "that row is no longer part of this page in this read view",
        ),
        E::InvalidQuery | E::InvalidParameterName(_) => {
            io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
        }
        other => io::Error::other(other.to_string()),
    }
}

struct Reaper {
    services: Mutex<Vec<Weak<Inner>>>,
}

impl Reaper {
    fn register(&self, inner: &Arc<Inner>) {
        let mut services = self.services.lock().unwrap_or_else(|e| e.into_inner());
        services.retain(|weak| weak.strong_count() > 0);
        services.push(Arc::downgrade(inner));
    }
}

fn reaper() -> &'static Arc<Reaper> {
    static REAPER: OnceLock<Arc<Reaper>> = OnceLock::new();
    REAPER.get_or_init(|| {
        let reaper = Arc::new(Reaper {
            services: Mutex::new(Vec::new()),
        });
        let sweeping = Arc::clone(&reaper);
        // One thread for the whole process: a view with no further requests
        // still has to release its read transaction. The interval is short
        // enough that a 30-second idle limit is observed closely, and the
        // thread only holds a mutex for the length of a sweep.
        let _ = std::thread::Builder::new()
            .name("lyrel-sqlite-views".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let services = {
                        let mut services =
                            sweeping.services.lock().unwrap_or_else(|e| e.into_inner());
                        services.retain(|weak| weak.strong_count() > 0);
                        services
                            .iter()
                            .filter_map(Weak::upgrade)
                            .collect::<Vec<_>>()
                    };
                    for service in services {
                        service.sweep();
                    }
                }
            });
        reaper
    })
}

impl SqliteEngine for SqliteService {
    fn open(&self, path: &Path, cancel: &SqliteCancel) -> io::Result<SqliteOpened> {
        self.open(path, cancel)
    }

    fn columns(
        &self,
        view: u64,
        table: &str,
        cancel: &SqliteCancel,
    ) -> io::Result<SqliteColumns> {
        self.columns(view, table, cancel)
    }

    fn count(&self, view: u64, query: &SqliteQuery, cancel: &SqliteCancel) -> io::Result<u64> {
        self.count(view, query, cancel)
    }

    fn page(
        &self,
        view: u64,
        query: &SqliteQuery,
        cancel: &SqliteCancel,
    ) -> io::Result<SqlitePage> {
        self.page(view, query, cancel)
    }

    fn value(
        &self,
        view: u64,
        query: &SqliteQuery,
        row: u32,
        column: u32,
        offset: u64,
        max_bytes: u64,
        cancel: &SqliteCancel,
    ) -> io::Result<SqliteValue> {
        self.value(view, query, row, column, offset, max_bytes, cancel)
    }

    fn close(&self, view: u64) {
        // The boolean reports whether a view was actually retired; the Host
        // seam only promises the idempotent, non-blocking close.
        let _ = self.close(view);
    }
}
