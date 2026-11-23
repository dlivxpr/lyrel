use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

use crate::daemon::control::{
    ControlClient, ControlEvent, ControlHello, ControlHelloOk, ControlRequest, EventSink,
    GIT_STREAM_IDLE_TIMEOUT, KEEPALIVE_DEAD_AFTER, KEEPALIVE_IDLE_BEFORE_PING,
    KEEPALIVE_PING_INTERVAL, LinkShutdown, ReplyOk,
};
use crate::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, HostRead, Meta, Output,
    SearchHit, SharedHost, ShellInventory, WatchHandle, WatchSub, guard_off_ui,
};

pub struct RemoteHost {
    id: HostId,
    client: Arc<ControlClient>,
    separator: char,
    watches: Arc<WatchRegistry>,
    streams: Arc<GitStreamRegistry>,
    next_stream: AtomicU64,
}

impl RemoteHost {
    pub fn connect<R, W>(
        r: R,
        w: W,
        connection_key: &str,
        hello: &ControlHello,
    ) -> io::Result<Arc<RemoteHost>>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        Self::connect_with(r, w, None, connection_key, hello)
    }

    pub fn over_tcp(
        sock: std::net::TcpStream,
        connection_key: &str,
        hello: &ControlHello,
    ) -> io::Result<Arc<RemoteHost>> {
        let r = sock.try_clone()?;
        let closer: Arc<dyn LinkShutdown> = Arc::new(sock.try_clone()?);
        Self::connect_with(r, sock, Some(closer), connection_key, hello)
    }

    #[cfg(unix)]
    pub fn over_unix(
        sock: std::os::unix::net::UnixStream,
        connection_key: &str,
        hello: &ControlHello,
    ) -> io::Result<Arc<RemoteHost>> {
        let r = sock.try_clone()?;
        let closer: Arc<dyn LinkShutdown> = Arc::new(sock.try_clone()?);
        Self::connect_with(r, sock, Some(closer), connection_key, hello)
    }

    pub fn connect_with<R, W>(
        r: R,
        w: W,
        shutdown: Option<Arc<dyn LinkShutdown>>,
        connection_key: &str,
        hello: &ControlHello,
    ) -> io::Result<Arc<RemoteHost>>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let watches = Arc::new(WatchRegistry::default());
        let sink_watches = Arc::clone(&watches);
        let streams: Arc<GitStreamRegistry> = Arc::new(GitStreamRegistry::default());
        let sink_streams = Arc::clone(&streams);
        let id = HostId::from_connection_key(connection_key);
        let sink: EventSink = Box::new(move |event| match event {
            ControlEvent::Watch { .. } | ControlEvent::WatchOverflow { .. } => {
                sink_watches.dispatch(event);
            }
            ControlEvent::GitChunk { .. } | ControlEvent::GitEnd { .. } => {
                sink_streams.dispatch(event);
            }
            other => crate::daemon::control::observe_event(id, other),
        });

        let client = Arc::new(ControlClient::connect_with(r, w, shutdown, hello, sink)?);
        let separator = client.hello().separator;
        let down_streams = Arc::clone(&streams);
        client.on_link_down(move || down_streams.close_all());

        let host = Arc::new(RemoteHost {
            id,
            streams,
            next_stream: AtomicU64::new(1),
            client: Arc::clone(&client),
            separator,
            watches,
        });
        spawn_keepalive(Arc::downgrade(&client));
        Ok(host)
    }

    pub fn peer(&self) -> &ControlHelloOk {
        self.client.hello()
    }

    pub fn home(&self) -> PathBuf {
        PathBuf::from(&self.client.hello().home)
    }

    pub fn client(&self) -> &Arc<ControlClient> {
        &self.client
    }

    pub fn into_shared(self: Arc<Self>) -> SharedHost {
        self
    }

    fn call(&self, req: ControlRequest) -> io::Result<ReplyOk> {
        self.client.call(req)
    }

    /// Send one SQLite request, or refuse before sending when the peer never
    /// announced the capability.
    fn sqlite_call(
        &self,
        request: crate::host::sqlite::SqliteRequest,
        cancel: &crate::host::sqlite::SqliteCancel,
    ) -> io::Result<crate::host::sqlite::SqliteReply> {
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::SQLITE_BROWSE)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the lyrel-server on this host (build {}) cannot browse SQLite databases; update lyrel-server",
                    self.peer().build
                ),
            ));
        }
        let got = self.client.call_full_with_cancel(
            ControlRequest::Sqlite {
                request: Box::new(request),
            },
            &[],
            cancel,
        )?;
        match got.reply {
            ReplyOk::Sqlite(reply) => Ok(*reply),
            other => Err(wrong_shape("a SQLite reply", &other)),
        }
    }
}

fn wire_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn wire_paths(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| wire_path(p)).collect()
}

fn wrong_shape(expected: &str, got: &ReplyOk) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("control peer answered with {got:?} where {expected} was required"),
    )
}

impl Host for RemoteHost {
    fn id(&self) -> HostId {
        self.id
    }

    fn host_metrics(&self) -> io::Result<crate::host::metrics::HostMetrics> {
        guard_off_ui();
        self.client.host_metrics()
    }

    fn disk_usage(&self, dir: &Path) -> io::Result<crate::host::disk::DiskUsage> {
        guard_off_ui();
        self.client.disk_usage(&dir.to_string_lossy().into_owned())
    }

    fn separator(&self) -> char {
        self.separator
    }

    fn is_absolute(&self, p: &Path) -> bool {
        let s = p.to_string_lossy();
        if self.separator == '\\' {
            let mut c = s.chars();
            let drive = matches!(
                (c.next(), c.next(), c.next()),
                (Some(a), Some(':'), Some('\\' | '/')) if a.is_ascii_alphabetic()
            );
            drive || s.starts_with("\\\\") || s.starts_with('\\') || s.starts_with('/')
        } else {
            s.starts_with('/')
        }
    }

    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        match self.call(ControlRequest::ReadDir {
            dir: wire_path(dir),
            root: root.map(wire_path),
        })? {
            ReplyOk::Entries(e) => Ok(e),
            other => Err(wrong_shape("a directory listing", &other)),
        }
    }

    fn stat(&self, p: &Path) -> io::Result<Meta> {
        match self.call(ControlRequest::Stat { path: wire_path(p) })? {
            ReplyOk::Meta(m) => Ok(m),
            other => Err(wrong_shape("file metadata", &other)),
        }
    }

    fn exists(&self, p: &Path) -> bool {
        matches!(
            self.call(ControlRequest::Exists { path: wire_path(p) }),
            Ok(ReplyOk::Bool(true))
        )
    }

    fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        let got = self.client.call_full(
            ControlRequest::ReadFile {
                path: wire_path(p),
                max_bytes,
            },
            &[],
        )?;
        match got.reply {
            ReplyOk::FileMeta { .. } => Ok(got.blob),
            other => Err(wrong_shape("file contents", &other)),
        }
    }

    /// A peer without [`crate::daemon::control::feature::FILE_RANGE`] is asked
    /// for nothing: sending it a request it has never heard of would take the
    /// whole link down, and reading the same path again per segment under a
    /// `(len, mtime)` guess is not the same version.
    fn open_read(&self, p: &Path) -> io::Result<Box<dyn HostRead>> {
        guard_off_ui();
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::FILE_RANGE)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the lyrel-server on this host (build {}) cannot stream one file version; update lyrel-server",
                    self.peer().build
                ),
            ));
        }
        match self.call(ControlRequest::OpenRead { path: wire_path(p) })? {
            ReplyOk::ReadOpened { session, meta } => Ok(Box::new(RemoteRead {
                client: Arc::clone(&self.client),
                session,
                meta,
                offset: 0,
                closed: false,
            })),
            other => Err(wrong_shape("an opened read session", &other)),
        }
    }

    fn sqlite_open(
        &self,
        path: &Path,
        cancel: &crate::host::sqlite::SqliteCancel,
    ) -> io::Result<crate::host::sqlite::SqliteOpened> {
        guard_off_ui();
        match self.sqlite_call(
            crate::host::sqlite::SqliteRequest::Open {
                path: wire_path(path),
            },
            cancel,
        )? {
            crate::host::sqlite::SqliteReply::Opened(opened) => Ok(opened),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "control peer answered with {other:?} where an opened SQLite view was required"
                ),
            )),
        }
    }

    fn sqlite_columns(
        &self,
        view: u64,
        table: &str,
        cancel: &crate::host::sqlite::SqliteCancel,
    ) -> io::Result<crate::host::sqlite::SqliteColumns> {
        guard_off_ui();
        match self.sqlite_call(
            crate::host::sqlite::SqliteRequest::Columns {
                view,
                table: table.to_string(),
            },
            cancel,
        )? {
            crate::host::sqlite::SqliteReply::Columns(columns) => Ok(columns),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control peer answered with {other:?} where SQLite columns were required"),
            )),
        }
    }

    fn sqlite_count(
        &self,
        view: u64,
        query: &crate::host::sqlite::SqliteQuery,
        cancel: &crate::host::sqlite::SqliteCancel,
    ) -> io::Result<u64> {
        guard_off_ui();
        match self.sqlite_call(
            crate::host::sqlite::SqliteRequest::Count {
                view,
                query: query.clone(),
            },
            cancel,
        )? {
            crate::host::sqlite::SqliteReply::Count(total) => Ok(total),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "control peer answered with {other:?} where a SQLite row count was required"
                ),
            )),
        }
    }

    fn sqlite_page(
        &self,
        view: u64,
        query: &crate::host::sqlite::SqliteQuery,
        cancel: &crate::host::sqlite::SqliteCancel,
    ) -> io::Result<crate::host::sqlite::SqlitePage> {
        guard_off_ui();
        match self.sqlite_call(
            crate::host::sqlite::SqliteRequest::Page {
                view,
                query: query.clone(),
            },
            cancel,
        )? {
            crate::host::sqlite::SqliteReply::Page(page) => Ok(page),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control peer answered with {other:?} where a SQLite page was required"),
            )),
        }
    }

    fn sqlite_value(
        &self,
        view: u64,
        query: &crate::host::sqlite::SqliteQuery,
        cell: crate::host::sqlite::SqliteCellRead,
        cancel: &crate::host::sqlite::SqliteCancel,
    ) -> io::Result<crate::host::sqlite::SqliteValue> {
        guard_off_ui();
        let crate::host::sqlite::SqliteCellRead {
            row,
            column,
            offset,
            max_bytes,
        } = cell;
        match self.sqlite_call(
            crate::host::sqlite::SqliteRequest::Value {
                view,
                query: query.clone(),
                row,
                column,
                offset,
                max_bytes,
            },
            cancel,
        )? {
            crate::host::sqlite::SqliteReply::Value(value) => Ok(value),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control peer answered with {other:?} where a SQLite value was required"),
            )),
        }
    }

    fn sqlite_close(&self, view: u64) {
        // Posted, not awaited: a document can be closed on the UI thread, and
        // the peer also closes every view when the connection ends. A peer
        // without the capability has nothing to close.
        if self
            .peer()
            .has_feature(crate::daemon::control::feature::SQLITE_BROWSE)
        {
            let _ = self.client.post(ControlRequest::Sqlite {
                request: Box::new(crate::host::sqlite::SqliteRequest::Close { view }),
            });
        }
    }

    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        match self.call(ControlRequest::Canonicalize { path: wire_path(p) })? {
            ReplyOk::Path(s) => Ok(PathBuf::from(s)),
            other => Err(wrong_shape("a canonical path", &other)),
        }
    }

    fn read_link(&self, p: &Path) -> io::Result<PathBuf> {
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::READ_LINK)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the lyrel-server on this host (build {}) cannot read link targets; update lyrel-server",
                    self.peer().build
                ),
            ));
        }
        match self.call(ControlRequest::ReadLink { path: wire_path(p) })? {
            ReplyOk::Path(target) => Ok(PathBuf::from(target)),
            other => Err(wrong_shape("a raw link target", &other)),
        }
    }

    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        show_hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        match self.call(ControlRequest::Search {
            roots: wire_paths(roots),
            query: query.to_string(),
            limit: limit as u64,
            max_dirs: max_dirs as u64,
            show_hidden,
        })? {
            ReplyOk::Hits(h) => Ok(h),
            other => Err(wrong_shape("search hits", &other)),
        }
    }

    /// Asked only of a peer that announces [`feature::CONTENT_SEARCH`]: one
    /// that predates the request cannot decode it, and a frame it cannot
    /// decode takes the whole link down. Such a peer answers `Unsupported`
    /// here without a byte on the wire, which the panel words as "update the
    /// server" rather than as a search that found nothing.
    ///
    /// [`feature::CONTENT_SEARCH`]: crate::daemon::control::feature::CONTENT_SEARCH
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &ContentQuery,
        limits: &ContentLimits,
    ) -> io::Result<ContentResults> {
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::CONTENT_SEARCH)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the lyrel-server on this host (build {}) cannot search file contents",
                    self.peer().build
                ),
            ));
        }
        match self.call(ControlRequest::SearchContent {
            roots: wire_paths(roots),
            query: query.clone(),
            limits: *limits,
        })? {
            ReplyOk::ContentHits(found) => Ok(found),
            other => Err(wrong_shape("content search results", &other)),
        }
    }

    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
        match self
            .client
            .call_with_blob(ControlRequest::WriteFile { path: wire_path(p) }, bytes)?
        {
            ReplyOk::Meta(m) => Ok(m),
            other => Err(wrong_shape("the written file's metadata", &other)),
        }
    }

    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.expect_unit(ControlRequest::CreateFileNew { path: wire_path(p) })
    }

    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.expect_unit(ControlRequest::CreateDir {
            path: wire_path(p),
            recursive,
        })
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.expect_unit(ControlRequest::Rename {
            from: wire_path(from),
            to: wire_path(to),
        })
    }

    fn link_rtt(&self) -> Option<std::time::Duration> {
        // A ping of our own rather than whatever the keepalive last left
        // behind: that one only fires on an idle link, and a link being polled
        // for this is by definition not idle, so its number would age out of
        // date exactly while someone is watching it.
        if self.client.is_connected()
            && let Err(e) = self.client.ping()
        {
            // The last good measurement below still stands. A link that has
            // just gone down reports the distance it had while it was up,
            // which is better than a blank until something notices it is gone.
            log::debug!("could not ping {:?}: {e}", self.id);
        }
        self.client.last_rtt()
    }

    /// The peer owns these panes' PTYs, so it is the one that can walk their
    /// process trees. A peer that does not announce the feature is not asked:
    /// it would answer `Err` and the caller cannot tell that apart from a pane
    /// serving nothing.
    fn pane_procs(&self, pane_id: u64) -> Option<crate::daemon::protocol::PaneProcs> {
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::PANE_PROCS)
        {
            return None;
        }
        match self.call(ControlRequest::PaneProcs { pane_id }) {
            Ok(ReplyOk::PaneProcs(procs)) => Some(procs),
            Ok(other) => {
                log::warn!("PaneProcs answered with {other:?}");
                None
            }
            Err(e) => {
                log::debug!("could not read pane {pane_id}'s processes: {e}");
                None
            }
        }
    }

    /// The peer owns the pane's processes, so it is the only one that can
    /// vouch for a termination scope. A peer without the feature answers
    /// `None` and the caller's monitor stays read-only, same as `pane_procs`.
    fn pane_end_process_plan(
        &self,
        pane_id: u64,
        pid: u32,
        started: Option<u64>,
    ) -> Option<Result<crate::daemon::protocol::EndProcessPlan, String>> {
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::PANE_END_PROCESS)
        {
            return None;
        }
        match self.call(ControlRequest::PaneEndProcessPlan {
            pane_id,
            pid,
            started,
        }) {
            Ok(ReplyOk::EndProcessPlan(plan)) => Some(Ok(plan)),
            Ok(other) => {
                log::warn!("PaneEndProcessPlan answered with {other:?}");
                None
            }
            // The refusal reason is information, not a broken link: the row
            // went stale between the poll and the click. Surface it so the
            // caller can say why instead of offering a dead action.
            Err(e) if crate::daemon::control::is_dialect_refusal(&e.to_string()) => {
                log::debug!("peer refused PaneEndProcessPlan: {e}");
                None
            }
            Err(e) => Some(Err(e.to_string())),
        }
    }

    fn pane_end_process(
        &self,
        pane_id: u64,
        scope: &[crate::daemon::protocol::ProcIdentity],
    ) -> Option<Vec<crate::daemon::protocol::EndProcessOutcome>> {
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::PANE_END_PROCESS)
        {
            return None;
        }
        match self.call(ControlRequest::PaneEndProcess {
            pane_id,
            scope: scope.to_vec(),
        }) {
            Ok(ReplyOk::EndProcessReport(outcomes)) => Some(outcomes),
            Ok(other) => {
                log::warn!("PaneEndProcess answered with {other:?}");
                None
            }
            Err(e) => {
                log::debug!("could not end pane {pane_id}'s processes: {e}");
                None
            }
        }
    }

    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.expect_unit(ControlRequest::Remove {
            path: wire_path(p),
            recursive,
        })
    }

    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        match self.call(ControlRequest::RepoRoot { path: wire_path(p) })? {
            ReplyOk::OptPath(s) => Ok(s.map(PathBuf::from)),
            other => Err(wrong_shape("a repository root", &other)),
        }
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> io::Result<Output> {
        match self.call(ControlRequest::Git {
            cwd: wire_path(cwd),
            args: args.iter().map(|a| a.to_string()).collect(),
        })? {
            ReplyOk::Output(o) => Ok(o),
            other => Err(wrong_shape("a process result", &other)),
        }
    }

    fn git_blob(&self, cwd: &Path, spec: &str, max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
        guard_off_ui();
        // A peer that never announced the request would take the link down
        // decoding it. Asking `git show` instead would transfer an unbounded
        // object, so the missing capability is the answer: the content-aware
        // Diff keeps its line fallback and names the reason.
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::GIT_BLOB)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the lyrel-server on this host (build {}) cannot send one revision-addressed file version; update lyrel-server",
                    self.peer().build
                ),
            ));
        }
        let got = self.client.call_full(
            ControlRequest::GitBlob {
                cwd: wire_path(cwd),
                spec: spec.to_string(),
                max_bytes,
            },
            &[],
        )?;
        match got.reply {
            ReplyOk::GitBlob { present, len } => match present {
                false => Ok(None),
                true => {
                    if len > max_bytes || got.blob.len() as u64 != len {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "control peer sent {} bytes for a {len}-byte file version (limit {max_bytes})",
                                got.blob.len()
                            ),
                        ));
                    }
                    Ok(Some(got.blob))
                }
            },
            other => Err(wrong_shape("one file version", &other)),
        }
    }

    fn git_with_deadline(
        &self,
        cwd: &Path,
        args: &[&str],
        deadline: Duration,
    ) -> io::Result<Output> {
        // Byte-for-byte the same request `git` sends; only the client's own
        // patience changes. The server never times a job out, so a v5 peer that
        // knows nothing about long git verbs serves this unchanged.
        let req = ControlRequest::Git {
            cwd: wire_path(cwd),
            args: args.iter().map(|a| a.to_string()).collect(),
        };
        match self.client.call_with_deadline(req, &[], deadline)?.reply {
            ReplyOk::Output(o) => Ok(o),
            other => Err(wrong_shape("a process result", &other)),
        }
    }

    fn git_lines(
        &self,
        cwd: &Path,
        args: &[&str],
        on_line: &mut dyn FnMut(&str),
    ) -> io::Result<Option<i32>> {
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        self.streams.insert(
            id,
            StreamSink {
                tx,
                queued: Arc::clone(&queued),
            },
        );
        let _guard = StreamGuard {
            streams: &self.streams,
            id,
        };

        match self.call(ControlRequest::GitStream {
            id,
            cwd: wire_path(cwd),
            args: args.iter().map(|a| a.to_string()).collect(),
        })? {
            ReplyOk::Unit => {}
            other => return Err(wrong_shape("an accepted git stream", &other)),
        }

        drain_git_stream(
            &mut |idle| rx.recv_timeout(idle),
            &queued,
            GIT_STREAM_IDLE_TIMEOUT,
            on_line,
        )
    }

    fn shells(&self) -> io::Result<ShellInventory> {
        match self.call(ControlRequest::Shells)? {
            ReplyOk::Shells(inv) => Ok(inv),
            other => Err(wrong_shape("a shell inventory", &other)),
        }
    }

    fn agent_environment(&self) -> io::Result<std::collections::BTreeMap<String, String>> {
        guard_off_ui();
        let mut environment = self.peer().agent_environment.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "update lyrel-server: this peer does not report its agent environment",
            )
        })?;
        environment.retain(|key, _| crate::host::is_agent_environment_key(key));
        Ok(environment)
    }

    fn agent_sessions(
        &self,
        directory: Option<&Path>,
    ) -> io::Result<Vec<crate::core::agent_history::PastSession>> {
        guard_off_ui();
        match self.call(ControlRequest::AgentSessions {
            directory: directory.map(|dir| dir.to_string_lossy().into_owned()),
        })? {
            ReplyOk::AgentSessions(sessions) => Ok(sessions),
            other => Err(wrong_shape("a list of agent sessions", &other)),
        }
    }

    fn session_operation(
        &self,
        request: &crate::host::session_operations::SessionOperation,
    ) -> Result<
        crate::host::session_operations::PreparedSession,
        crate::host::session_operations::SessionOperationError,
    > {
        use crate::host::session_operations::{SessionErrorKind, SessionOperationError};
        let error = |kind, message: String| SessionOperationError {
            kind,
            message,
            retained: None,
        };
        if request.expected_host != self.id.0 {
            return Err(error(
                SessionErrorKind::WrongHost,
                "session belongs to another host".into(),
            ));
        }
        if !self.is_connected() {
            return Err(error(
                SessionErrorKind::Connection,
                "session host is disconnected".into(),
            ));
        }
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::SESSION_OPERATIONS)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                "update lyrel-server: this peer does not support session operations".into(),
            ));
        }
        // The resume selector, the store check and the launched argv are
        // Antigravity-specific. A peer without the capability would either
        // not know the operation or not perform it, so answer here instead of
        // risking a control link that cannot decode or honour the request.
        if request.agent == crate::core::cli_agent::CLIAgent::Antigravity
            && !self
                .peer()
                .has_feature(crate::daemon::control::feature::ANTIGRAVITY)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                format!(
                    "update lyrel-server: antigravity-v1 is required to resume {} conversations on this host",
                    request.agent.display_name()
                ),
            ));
        }
        let mut wire = request.clone();
        wire.expected_host = 0;
        let reply = self.call(ControlRequest::SessionOperation { request: Box::new(wire) })
            .map_err(|e| error(SessionErrorKind::Connection, format!("session preparation connection failed: {e}; if this was a fork, native creation may already have succeeded — inspect its history before repeating")))?;
        let ReplyOk::SessionOperation(result) = reply else {
            return Err(error(
                SessionErrorKind::Identity,
                "session host returned an incompatible response".into(),
            ));
        };
        match *result {
            Ok(mut prepared) => {
                if prepared.agent != request.agent
                    || prepared.source_id != request.source_id
                    || prepared.host_id != 0
                {
                    return Err(error(
                        SessionErrorKind::Identity,
                        "session host returned a different operation identity".into(),
                    ));
                }
                prepared.host_id = self.id.0;
                Ok(prepared)
            }
            Err(mut error) => {
                if let Some(prepared) = &mut error.retained {
                    prepared.host_id = self.id.0;
                }
                Err(error)
            }
        }
    }

    fn opencode_sessions(
        &self,
        request: &crate::host::session_operations::SessionOperation,
    ) -> Result<
        Vec<crate::core::agent_history::PastSession>,
        crate::host::session_operations::SessionOperationError,
    > {
        use crate::host::session_operations::{SessionErrorKind, SessionOperationError};
        let error = |kind, message: &str| SessionOperationError {
            kind,
            message: message.into(),
            retained: None,
        };
        if request.expected_host != self.id.0 {
            return Err(error(
                SessionErrorKind::WrongHost,
                "OpenCode history belongs to another Host",
            ));
        }
        if !self.is_connected() {
            return Err(error(
                SessionErrorKind::Connection,
                "OpenCode history Host is disconnected",
            ));
        }
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::SESSION_OPERATIONS)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                "update lyrel-server to query scoped OpenCode history",
            ));
        }
        let mut wire = request.clone();
        wire.expected_host = 0;
        match self
            .call(ControlRequest::OpenCodeSessions {
                request: Box::new(wire),
            })
            .map_err(|_| {
                error(
                    SessionErrorKind::Connection,
                    "OpenCode history Host did not answer",
                )
            })? {
            ReplyOk::OpenCodeSessions(result) => *result,
            _ => Err(error(
                SessionErrorKind::Identity,
                "OpenCode history Host returned an incompatible response",
            )),
        }
    }

    fn opencode_statistics(
        &self,
        request: &crate::host::session_operations::SessionOperation,
    ) -> Result<
        crate::host::session_operations::opencode::OpenCodeStatistics,
        crate::host::session_operations::SessionOperationError,
    > {
        use crate::host::session_operations::{SessionErrorKind, SessionOperationError};
        let error = |kind, message: &str| SessionOperationError {
            kind,
            message: message.into(),
            retained: None,
        };
        if request.expected_host != self.id.0 {
            return Err(error(
                SessionErrorKind::WrongHost,
                "OpenCode statistics belongs to another Host",
            ));
        }
        if !self.is_connected() {
            return Err(error(
                SessionErrorKind::Connection,
                "OpenCode statistics Host is disconnected",
            ));
        }
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::OPENCODE_STATISTICS)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                "update lyrel-server: opencode-statistics-v1 is required",
            ));
        }
        let mut wire = request.clone();
        wire.expected_host = 0;
        match self
            .call(ControlRequest::OpenCodeStatistics {
                request: Box::new(wire),
            })
            .map_err(|_| {
                error(
                    SessionErrorKind::Connection,
                    "OpenCode statistics Host did not answer",
                )
            })? {
            ReplyOk::OpenCodeStatistics(result) => *result,
            _ => Err(error(
                SessionErrorKind::Identity,
                "OpenCode statistics Host returned an incompatible response",
            )),
        }
    }

    fn hermes_sessions(
        &self,
        request: &crate::host::session_operations::SessionOperation,
    ) -> Result<
        Vec<crate::core::agent_history::PastSession>,
        crate::host::session_operations::SessionOperationError,
    > {
        use crate::host::session_operations::{SessionErrorKind, SessionOperationError};
        let error = |kind, message: &str| SessionOperationError {
            kind,
            message: message.into(),
            retained: None,
        };
        if request.expected_host != self.id.0 {
            return Err(error(
                SessionErrorKind::WrongHost,
                "Hermes history belongs to another Host",
            ));
        }
        if !self.is_connected() {
            return Err(error(
                SessionErrorKind::Connection,
                "Hermes history Host is disconnected",
            ));
        }
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::HERMES_HISTORY)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                "update lyrel-server to query scoped Hermes history",
            ));
        }
        let mut wire = request.clone();
        wire.expected_host = 0;
        match self
            .call(ControlRequest::HermesSessions {
                request: Box::new(wire),
            })
            .map_err(|_| {
                error(
                    SessionErrorKind::Connection,
                    "Hermes history Host did not answer",
                )
            })? {
            ReplyOk::HermesSessions(result) => *result,
            _ => Err(error(
                SessionErrorKind::Identity,
                "Hermes history Host returned an incompatible response",
            )),
        }
    }

    fn hermes_statistics(
        &self,
        request: &crate::host::session_operations::SessionOperation,
    ) -> Result<serde_json::Value, crate::host::session_operations::SessionOperationError> {
        use crate::host::session_operations::{SessionErrorKind, SessionOperationError};
        let error = |kind, message: &str| SessionOperationError {
            kind,
            message: message.into(),
            retained: None,
        };
        if request.expected_host != self.id.0 {
            return Err(error(
                SessionErrorKind::WrongHost,
                "Hermes statistics belongs to another Host",
            ));
        }
        if !self.is_connected() {
            return Err(error(
                SessionErrorKind::Connection,
                "Hermes statistics Host is disconnected",
            ));
        }
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::HERMES_STATISTICS)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                "update lyrel-server for hermes-statistics-v1",
            ));
        }
        let mut wire = request.clone();
        wire.expected_host = 0;
        match self
            .call(ControlRequest::HermesStatistics {
                request: Box::new(wire),
            })
            .map_err(|_| {
                error(
                    SessionErrorKind::Connection,
                    "Hermes statistics Host did not answer",
                )
            })? {
            ReplyOk::HermesStatistics(result) => *result,
            _ => Err(error(
                SessionErrorKind::Identity,
                "Hermes statistics Host returned an incompatible response",
            )),
        }
    }

    /// The bounded batch runs on the data-owning Host: the route, the
    /// connection and the negotiated capability are revalidated here, and a
    /// peer without `session-batch-v1` sees an explicit Unsupported instead
    /// of a request it cannot decode taking the link down.
    fn session_batch(
        &self,
        request: &crate::host::session_operations::batch::SessionBatchRequest,
    ) -> Result<
        crate::host::session_operations::batch::SessionBatchReply,
        crate::host::session_operations::SessionOperationError,
    > {
        use crate::host::session_operations::{SessionErrorKind, SessionOperationError};
        let error = |kind, message: &str| SessionOperationError {
            kind,
            message: message.into(),
            retained: None,
        };
        if request.expected_host() != self.id.0 {
            return Err(error(
                SessionErrorKind::WrongHost,
                "session batch belongs to another Host",
            ));
        }
        if !self.is_connected() {
            return Err(error(
                SessionErrorKind::Connection,
                "session batch Host is disconnected",
            ));
        }
        if !self
            .peer()
            .has_feature(crate::daemon::control::feature::SESSION_BATCH)
        {
            return Err(error(
                SessionErrorKind::Unsupported,
                "update lyrel-server for session-batch-v1",
            ));
        }
        let mut wire = request.clone();
        match &mut wire {
            crate::host::session_operations::batch::SessionBatchRequest::Prepare(request) => {
                request.expected_host = 0;
            }
            crate::host::session_operations::batch::SessionBatchRequest::Execute(request) => {
                request.expected_host = 0;
            }
        }
        match self
            .call(ControlRequest::SessionBatch {
                request: Box::new(wire),
            })
            .map_err(|e| {
                error(
                    SessionErrorKind::Connection,
                    &format!("session batch connection failed: {e}"),
                )
            })? {
            ReplyOk::SessionBatch(result) => *result,
            _ => Err(error(
                SessionErrorKind::Identity,
                "session batch Host returned an incompatible response",
            )),
        }
    }

    fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
        let id = match self.call(ControlRequest::WatchOpen {
            dirs: wire_paths(dirs),
        })? {
            ReplyOk::WatchId(id) => id,
            other => return Err(wrong_shape("a watch id", &other)),
        };

        let (tx, rx) = smol::channel::unbounded();
        self.watches.insert(id, tx, dirs.to_vec());

        Ok(WatchSub::new(
            rx,
            Box::new(RemoteWatch {
                id,
                client: Arc::clone(&self.client),
                watches: Arc::clone(&self.watches),
            }),
        ))
    }

    fn is_connected(&self) -> bool {
        self.client.is_connected()
    }
}

impl RemoteHost {
    fn expect_unit(&self, req: ControlRequest) -> io::Result<()> {
        match self.call(req)? {
            ReplyOk::Unit => Ok(()),
            other => Err(wrong_shape("an acknowledgement", &other)),
        }
    }
}

impl std::fmt::Debug for RemoteHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHost")
            .field("id", &self.id)
            .field("separator", &self.separator)
            .field("connected", &self.is_connected())
            .finish()
    }
}

const GIT_STREAM_QUEUE_BUDGET: usize = 32 * 1024 * 1024;

type NextGitMsg<'a> = &'a mut dyn FnMut(Duration) -> Result<GitStreamMsg, mpsc::RecvTimeoutError>;

fn drain_git_stream(
    next: NextGitMsg<'_>,
    queued: &AtomicUsize,
    idle: Duration,
    on_line: &mut dyn FnMut(&str),
) -> io::Result<Option<i32>> {
    let mut split = crate::core::git::LineSplitter::default();
    loop {
        match next(idle) {
            Ok(GitStreamMsg::Chunk(bytes)) => {
                let _ = queued.try_update(Ordering::AcqRel, Ordering::Acquire, |q| {
                    Some(q.saturating_sub(bytes.len()))
                });
                split.push(&bytes, &mut *on_line);
            }
            Ok(GitStreamMsg::Overrun) => {
                return Err(io::Error::other(format!(
                    "the git stream outran this client by more than \
                     {GIT_STREAM_QUEUE_BUDGET} queued bytes"
                )));
            }
            Ok(GitStreamMsg::End { code, failed }) => {
                split.finish(&mut *on_line);
                return if failed {
                    Err(io::Error::other("the server could not run git"))
                } else {
                    Ok(code)
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the git stream went silent for {idle:?} while the link stayed up"),
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("the control connection closed mid-stream"));
            }
        }
    }
}

struct StreamGuard<'a> {
    streams: &'a GitStreamRegistry,
    id: u64,
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        self.streams.remove(self.id);
    }
}

enum GitStreamMsg {
    Chunk(Vec<u8>),
    End { code: Option<i32>, failed: bool },
    Overrun,
}

struct StreamSink {
    tx: mpsc::Sender<GitStreamMsg>,
    queued: Arc<AtomicUsize>,
}

#[derive(Default)]
struct GitStreamRegistry {
    streams: Mutex<StreamTable>,
}

#[derive(Default)]
struct StreamTable {
    senders: HashMap<u64, StreamSink>,
    closed: bool,
}

impl GitStreamRegistry {
    fn insert(&self, id: u64, sink: StreamSink) {
        if let Ok(mut m) = self.streams.lock()
            && !m.closed
        {
            m.senders.insert(id, sink);
        }
    }

    fn remove(&self, id: u64) {
        if let Ok(mut m) = self.streams.lock() {
            m.senders.remove(&id);
        }
    }

    fn close_all(&self) {
        let Ok(mut m) = self.streams.lock() else {
            return;
        };
        m.closed = true;
        m.senders.clear();
    }

    fn dispatch(&self, event: ControlEvent) {
        let (id, msg) = match event {
            ControlEvent::GitChunk { id, bytes } => (id, GitStreamMsg::Chunk(bytes)),
            ControlEvent::GitEnd { id, code, failed } => (id, GitStreamMsg::End { code, failed }),
            _ => return,
        };
        let Ok(mut m) = self.streams.lock() else {
            return;
        };
        let over = match (m.senders.get(&id), &msg) {
            (None, _) => return,
            (Some(sink), GitStreamMsg::Chunk(bytes)) => {
                sink.queued.fetch_add(bytes.len(), Ordering::AcqRel) + bytes.len()
                    > GIT_STREAM_QUEUE_BUDGET
            }
            (Some(_), _) => false,
        };
        if over {
            if let Some(sink) = m.senders.remove(&id) {
                let _ = sink.tx.send(GitStreamMsg::Overrun);
            }
            return;
        }
        if let Some(sink) = m.senders.get(&id) {
            let _ = sink.tx.send(msg);
        }
    }
}

#[derive(Default)]
struct WatchRegistry {
    subs: Mutex<HashMap<u64, WatchEntry>>,
}

struct WatchEntry {
    tx: smol::channel::Sender<Vec<PathBuf>>,
    dirs: Vec<PathBuf>,
}

impl WatchRegistry {
    fn insert(&self, id: u64, tx: smol::channel::Sender<Vec<PathBuf>>, dirs: Vec<PathBuf>) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.insert(id, WatchEntry { tx, dirs });
        }
    }

    fn set_dirs(&self, id: u64, dirs: Vec<PathBuf>) {
        if let Ok(mut subs) = self.subs.lock()
            && let Some(entry) = subs.get_mut(&id)
        {
            entry.dirs = dirs;
        }
    }

    fn remove(&self, id: u64) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.remove(&id);
        }
    }

    fn dispatch(&self, event: ControlEvent) {
        let (id, paths) = match event {
            ControlEvent::Watch { id, paths } => {
                (id, paths.into_iter().map(PathBuf::from).collect::<Vec<_>>())
            }
            ControlEvent::WatchOverflow { id } => {
                let dirs = self
                    .subs
                    .lock()
                    .ok()
                    .and_then(|s| s.get(&id).map(|e| e.dirs.clone()))
                    .unwrap_or_default();
                (id, dirs)
            }
            other => {
                log::trace!("control event not routed by RemoteHost: {other:?}");
                return;
            }
        };

        if paths.is_empty() {
            return;
        }
        let Ok(subs) = self.subs.lock() else { return };
        if let Some(entry) = subs.get(&id) {
            let _ = entry.tx.try_send(paths);
        }
    }
}

/// A bound read session on the peer, one `ReadChunk` per segment.
struct RemoteRead {
    client: Arc<ControlClient>,
    session: u64,
    meta: Meta,
    offset: u64,
    closed: bool,
}

impl HostRead for RemoteRead {
    fn meta(&self) -> Meta {
        self.meta
    }

    fn read_segment(&mut self, max_bytes: u64) -> io::Result<Vec<u8>> {
        guard_off_ui();
        if max_bytes == 0 || max_bytes > crate::host::transfer::SEGMENT_BYTES_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "a read segment must be 1..={} bytes, not {max_bytes}",
                    crate::host::transfer::SEGMENT_BYTES_MAX
                ),
            ));
        }
        let remaining = self.meta.len.saturating_sub(self.offset);
        if remaining == 0 {
            return Ok(Vec::new());
        }
        let want = max_bytes.min(remaining);
        let got = self.client.call_full(
            ControlRequest::ReadChunk {
                session: self.session,
                max_bytes: want,
            },
            &[],
        )?;
        match got.reply {
            ReplyOk::FileMeta { meta } => {
                if meta != self.meta {
                    self.closed = true;
                    return Err(crate::host::read::file_changed(
                        "the peer answered a segment with a different file version",
                    ));
                }
                if got.blob.len() as u64 > want {
                    self.closed = true;
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "the peer returned {} bytes for a {want}-byte segment",
                            got.blob.len()
                        ),
                    ));
                }
                if got.blob.is_empty() {
                    self.closed = true;
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the peer ended a read session before its own version length",
                    ));
                }
                self.offset += got.blob.len() as u64;
                Ok(got.blob)
            }
            other => {
                self.closed = true;
                Err(wrong_shape("the next file segment", &other))
            }
        }
    }
}

impl Drop for RemoteRead {
    fn drop(&mut self) {
        // Posted, not called: a preview can be released on the UI thread, and
        // the peer frees the session on its own once this arrives (or when the
        // connection ends).
        if !self.closed && self.client.is_connected() {
            let _ = self.client.post(ControlRequest::CloseRead {
                session: self.session,
            });
        }
    }
}

struct RemoteWatch {
    id: u64,
    client: Arc<ControlClient>,
    watches: Arc<WatchRegistry>,
}

impl WatchHandle for RemoteWatch {
    fn set_dirs(&self, dirs: &[PathBuf]) -> io::Result<()> {
        match self.client.call(ControlRequest::WatchSet {
            id: self.id,
            dirs: wire_paths(dirs),
        })? {
            ReplyOk::Unit => {
                self.watches.set_dirs(self.id, dirs.to_vec());
                Ok(())
            }
            other => Err(wrong_shape("an acknowledgement", &other)),
        }
    }
}

impl Drop for RemoteWatch {
    fn drop(&mut self) {
        self.watches.remove(self.id);
        // Posted, not called: the last handle is often let go on the UI
        // thread — a watcher whose repository changed, an open that landed
        // after its subscription moved on — and waiting there for the peer's
        // acknowledgement froze every window for a round trip each time.
        if self.client.is_connected() {
            let _ = self.client.post(ControlRequest::WatchClose { id: self.id });
        }
    }
}

fn spawn_keepalive(client: Weak<ControlClient>) {
    let _ = std::thread::Builder::new()
        .name("lyrel-control-keepalive".into())
        .spawn(move || {
            let mut last_ping = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(client) = client.upgrade() else {
                    return;
                };
                if !client.is_connected() {
                    return;
                }
                let idle = client.idle_for();
                if idle >= KEEPALIVE_DEAD_AFTER {
                    log::warn!("control link silent for {idle:?}; treating it as dead",);
                    client.close();
                    return;
                }
                if idle >= KEEPALIVE_IDLE_BEFORE_PING
                    && last_ping.elapsed() >= KEEPALIVE_PING_INTERVAL
                {
                    last_ping = Instant::now();
                    if let Err(e) = client.ping() {
                        log::debug!("control keepalive ping failed: {e}");
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::control::{
        ControlClientMsg, ControlReply, ControlServerMsg, MTime, WireError, WireErrorKind,
    };
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    fn hello_ok(separator: char) -> ControlHelloOk {
        ControlHelloOk {
            control_version: crate::daemon::control::CONTROL_VERSION,
            protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
            build: "test".into(),
            separator,
            home: "/home/me".into(),
            agent_environment: None,
            features: vec![
                crate::daemon::control::feature::CONTROL.into(),
                crate::daemon::control::feature::HOST_RPC.into(),
            ],
            instance: "test-instance".into(),
        }
    }

    fn meta() -> Meta {
        Meta {
            is_dir: false,
            is_symlink: false,
            len: 12,
            mtime: Some(MTime {
                secs: 1_769_000_000,
                nanos: 7,
            }),
            readonly: false,
        }
    }

    fn host_with_peer<F>(
        separator: char,
        answer: F,
    ) -> (Arc<RemoteHost>, mpsc::Receiver<ControlRequest>)
    where
        F: Fn(&ControlRequest) -> Option<(ControlReply, Vec<u8>)> + Send + 'static,
    {
        host_with_peer_announcing(separator, &[], answer)
    }

    #[test]
    fn sqlite_is_refused_before_send_when_the_peer_lacks_the_feature() {
        let (host, seen) = host_with_peer_announcing('/', &[], |_| None);
        let error = host
            .sqlite_open(
                Path::new("/data.sqlite"),
                &crate::host::sqlite::SqliteCancel::default(),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("update lyrel-server"), "{error}");
        assert!(
            seen.try_recv().is_err(),
            "an older peer must not receive a request it has never heard of"
        );
    }

    #[test]
    fn sqlite_unsupported_from_a_feature_peer_is_not_an_empty_schema() {
        use crate::host::sqlite::SqliteCancel;
        let (host, _) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::SQLITE_BROWSE],
            |req| match req {
                ControlRequest::Sqlite { .. } => Some((
                    ControlReply::Err(WireError::new(
                        WireErrorKind::Unsupported,
                        "this host serves no databases",
                    )),
                    Vec::new(),
                )),
                _ => None,
            },
        );
        let error = host
            .sqlite_open(Path::new("/data.sqlite"), &SqliteCancel::default())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn sqlite_replies_map_to_typed_host_values() {
        use crate::host::sqlite::{
            SqliteCancel, SqliteColumns, SqliteObject, SqliteObjectKind, SqliteOpened, SqlitePage,
            SqliteQuery, SqliteReply, SqliteRequest, SqliteValue,
        };
        let opened = SqliteOpened {
            view: 9,
            objects: vec![SqliteObject {
                name: "items".into(),
                kind: SqliteObjectKind::Table,
                sql: "CREATE TABLE items(id INTEGER)".into(),
            }],
            journal_mode: "wal".into(),
            idle_millis: 30_000,
            max_age_millis: 180_000,
        };
        let page = SqlitePage {
            page: 2,
            start_column: 1,
            rows: vec![vec![
                SqliteValue::Null,
                SqliteValue::Integer {
                    value: -9007199254740993,
                },
                SqliteValue::real(f64::NEG_INFINITY.to_bits()),
                SqliteValue::Text {
                    preview: vec![0xff, 0xfe, b'a'],
                    total: 3,
                },
                SqliteValue::Blob {
                    preview: vec![0x00, 0xff],
                    total: 8,
                },
            ]],
            more: true,
        };
        let (host, _) =
            host_with_peer_announcing('/', &[crate::daemon::control::feature::SQLITE_BROWSE], {
                let opened = opened.clone();
                let page = page.clone();
                let columns = SqliteColumns {
                    names: vec!["id".into()],
                    order: vec!["rowid".into()],
                };
                move |req| match req {
                    ControlRequest::Sqlite { request } => Some((
                        ControlReply::Ok(ReplyOk::Sqlite(Box::new(match &**request {
                            SqliteRequest::Open { .. } => SqliteReply::Opened(opened.clone()),
                            SqliteRequest::Columns { .. } => SqliteReply::Columns(columns.clone()),
                            SqliteRequest::Count { .. } => SqliteReply::Count(17),
                            SqliteRequest::Page { .. } => SqliteReply::Page(page.clone()),
                            _ => SqliteReply::Closed,
                        }))),
                        Vec::new(),
                    )),
                    _ => None,
                }
            });
        let cancel = SqliteCancel::default();
        let got = host
            .sqlite_open(Path::new("/data.sqlite"), &cancel)
            .unwrap();
        assert_eq!(got, opened);
        let query = SqliteQuery {
            table: "items".into(),
            columns: vec!["id".into()],
            order: vec!["rowid".into()],
            page: 2,
            start_column: 1,
            ..SqliteQuery::default()
        };
        assert_eq!(host.sqlite_count(got.view, &query, &cancel).unwrap(), 17);
        let got_page = host.sqlite_page(got.view, &query, &cancel).unwrap();
        assert_eq!(got_page, page);
        let columns = host.sqlite_columns(got.view, "items", &cancel).unwrap();
        assert_eq!(columns.names, ["id"]);
    }

    /// [`host_with_peer`], with a peer whose hello names `extra` features on
    /// top of the two every control peer has.
    fn host_with_peer_announcing<F>(
        separator: char,
        extra: &[&str],
        answer: F,
    ) -> (Arc<RemoteHost>, mpsc::Receiver<ControlRequest>)
    where
        F: Fn(&ControlRequest) -> Option<(ControlReply, Vec<u8>)> + Send + 'static,
    {
        let mut hello = hello_ok(separator);
        hello.features.extend(extra.iter().map(|f| f.to_string()));
        if extra.contains(&crate::daemon::control::feature::AGENT_ENVIRONMENT) {
            hello.agent_environment = Some(std::collections::BTreeMap::from([
                ("KIMI_CODE_HOME".into(), "/peer/隔离 scope".into()),
                ("OMP_PROFILE".into(), "".into()),
                ("ANTHROPIC_API_KEY".into(), "must-not-cross-host".into()),
                ("PATH".into(), "/must/not/replay".into()),
            ]));
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen_tx, seen_rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            match ControlClientMsg::read(&mut sock).unwrap() {
                ControlClientMsg::Hello(_) => {}
                other => panic!("expected Hello, got {other:?}"),
            }
            ControlServerMsg::HelloOk(hello).encode(&mut sock).unwrap();
            sock.flush().unwrap();
            loop {
                let (req_id, req) = match ControlClientMsg::read(&mut sock) {
                    Ok(ControlClientMsg::Request { req_id, req }) => (req_id, req),
                    Ok(ControlClientMsg::RequestBlob { req_id, req, blob }) => {
                        let _ = blob;
                        (req_id, req)
                    }
                    _ => return,
                };
                let reply = answer(&req);
                let _ = seen_tx.send(req);
                match reply {
                    Some((reply, blob)) if blob.is_empty() => {
                        ControlServerMsg::Response { req_id, reply }
                            .encode(&mut sock)
                            .unwrap();
                    }
                    Some((reply, blob)) => {
                        ControlServerMsg::ResponseBlob {
                            req_id,
                            reply,
                            blob,
                        }
                        .encode(&mut sock)
                        .unwrap();
                    }
                    None => return,
                }
                sock.flush().unwrap();
            }
        });

        let sock = TcpStream::connect(addr).unwrap();
        let host = RemoteHost::over_tcp(
            sock,
            "ssh-alias:testbox",
            &ControlHello::host_rpc("tok", "laptop"),
        )
        .unwrap();
        (host, seen_rx)
    }

    fn read_meta(len: u64) -> Meta {
        Meta {
            is_dir: false,
            is_symlink: false,
            len,
            mtime: Some(MTime {
                secs: 1_769_000_000,
                nanos: 11,
            }),
            readonly: false,
        }
    }

    #[test]
    fn a_peer_without_file_range_is_never_asked_for_a_session() {
        let (host, seen) = host_with_peer_announcing('/', &[], |_| None);
        let error = match host.open_read(Path::new("/large.png")) {
            Ok(_) => panic!("a peer without the feature opened a segmented read"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("update lyrel-server"), "{error}");
        assert!(
            seen.try_recv().is_err(),
            "a request an old peer cannot decode must never be sent"
        );
    }

    #[test]
    fn a_bound_session_pulls_bounded_segments_and_closes_on_drop() {
        use crate::daemon::control::feature::FILE_RANGE;
        use crate::host::transfer::SEGMENT_BYTES;
        use std::sync::{Arc as StdArc, Mutex as StdMutex};

        let body: StdArc<Vec<u8>> =
            StdArc::new((0..(600 * 1024u32)).map(|i| (i % 251) as u8).collect());
        let total = body.len() as u64;
        let offset = StdArc::new(StdMutex::new(0u64));
        let served = StdArc::clone(&body);
        let (host, seen) = host_with_peer_announcing('/', &[FILE_RANGE], move |req| match req {
            ControlRequest::OpenRead { path } => {
                assert_eq!(path, "/large.bin");
                Some((
                    ControlReply::Ok(ReplyOk::ReadOpened {
                        session: 11,
                        meta: read_meta(total),
                    }),
                    Vec::new(),
                ))
            }
            ControlRequest::ReadChunk { session, max_bytes } => {
                assert_eq!(*session, 11);
                assert!(*max_bytes <= SEGMENT_BYTES);
                let mut offset = offset.lock().unwrap();
                let start = *offset as usize;
                let end = (start + *max_bytes as usize).min(served.len());
                *offset = end as u64;
                Some((
                    ControlReply::Ok(ReplyOk::FileMeta {
                        meta: read_meta(total),
                    }),
                    served[start..end].to_vec(),
                ))
            }
            ControlRequest::CloseRead { session } => {
                assert_eq!(*session, 11);
                Some((ControlReply::Ok(ReplyOk::Unit), Vec::new()))
            }
            other => panic!("unexpected request {other:?}"),
        });

        let mut progress = Vec::new();
        let outcome = crate::host::read::read_file_segmented(
            &*host,
            Path::new("/large.bin"),
            8 * 1024 * 1024,
            &crate::host::transfer::CancelToken::default(),
            |update| progress.push(update),
        )
        .unwrap();
        assert!(
            outcome.bytes == *body,
            "segments reassemble the bound version"
        );
        assert_eq!(
            progress.last().map(|p| (p.read, p.total)),
            Some((total, total))
        );

        let mut chunks = 0;
        let mut closed = false;
        while let Ok(request) = seen.recv_timeout(Duration::from_secs(2)) {
            match request {
                ControlRequest::OpenRead { .. } => {}
                ControlRequest::ReadChunk { .. } => chunks += 1,
                ControlRequest::CloseRead { .. } => {
                    closed = true;
                    break;
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
        assert!(chunks >= 2, "a 600 KiB file cannot be one segment");
        assert!(closed, "dropping the reader closes the far-side session");
    }

    #[test]
    fn a_changed_version_is_reported_not_retried() {
        use crate::daemon::control::feature::FILE_RANGE;

        let (host, _seen) = host_with_peer_announcing('/', &[FILE_RANGE], |req| match req {
            ControlRequest::OpenRead { .. } => Some((
                ControlReply::Ok(ReplyOk::ReadOpened {
                    session: 3,
                    meta: read_meta(4),
                }),
                Vec::new(),
            )),
            ControlRequest::ReadChunk { .. } => Some((
                ControlReply::Err(WireError::new(
                    WireErrorKind::FileChanged,
                    "changed under the read",
                )),
                Vec::new(),
            )),
            ControlRequest::CloseRead { .. } => Some((ControlReply::Ok(ReplyOk::Unit), Vec::new())),
            other => panic!("unexpected request {other:?}"),
        });

        let mut reader = host.open_read(Path::new("/x.png")).unwrap();
        let error = reader.read_segment(4).unwrap_err();
        assert!(crate::host::read::is_file_changed(&error), "{error}");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_read_session_fails_when_its_link_goes_down() {
        use crate::daemon::control::feature::FILE_RANGE;

        let (host, _seen) = host_with_peer_announcing('/', &[FILE_RANGE], |req| match req {
            ControlRequest::OpenRead { .. } => Some((
                ControlReply::Ok(ReplyOk::ReadOpened {
                    session: 6,
                    meta: read_meta(1024),
                }),
                Vec::new(),
            )),
            // Returning None drops the peer's socket.
            ControlRequest::ReadChunk { .. } => None,
            other => panic!("unexpected request {other:?}"),
        });
        let mut reader = host.open_read(Path::new("/lost.png")).unwrap();
        let error = reader.read_segment(16).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset, "{error}");
    }

    #[test]
    fn an_empty_segment_before_the_version_end_is_not_eof() {
        use crate::daemon::control::feature::FILE_RANGE;

        let (host, _seen) = host_with_peer_announcing('/', &[FILE_RANGE], |req| match req {
            ControlRequest::OpenRead { .. } => Some((
                ControlReply::Ok(ReplyOk::ReadOpened {
                    session: 4,
                    meta: read_meta(64),
                }),
                Vec::new(),
            )),
            ControlRequest::ReadChunk { .. } => Some((
                ControlReply::Ok(ReplyOk::FileMeta {
                    meta: read_meta(64),
                }),
                Vec::new(),
            )),
            other => panic!("unexpected request {other:?}"),
        });
        let mut reader = host.open_read(Path::new("/short.png")).unwrap();
        let error = reader.read_segment(64).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "{error}");
    }

    #[test]
    fn a_segment_from_another_version_is_refused() {
        use crate::daemon::control::feature::FILE_RANGE;

        let (host, _seen) = host_with_peer_announcing('/', &[FILE_RANGE], |req| match req {
            ControlRequest::OpenRead { .. } => Some((
                ControlReply::Ok(ReplyOk::ReadOpened {
                    session: 5,
                    meta: read_meta(64),
                }),
                Vec::new(),
            )),
            ControlRequest::ReadChunk { .. } => Some((
                ControlReply::Ok(ReplyOk::FileMeta {
                    meta: read_meta(65),
                }),
                vec![7; 64],
            )),
            other => panic!("unexpected request {other:?}"),
        });
        let mut reader = host.open_read(Path::new("/moved.png")).unwrap();
        let error = reader.read_segment(64).unwrap_err();
        assert!(crate::host::read::is_file_changed(&error), "{error}");
    }

    #[test]
    fn git_lines_streams_over_the_wire() {
        let (host, seen) = host_with_peer_streaming();

        let mut lines = Vec::new();
        let code = host
            .git_lines(Path::new("/repo"), &["diff"], &mut |l| {
                lines.push(l.to_string())
            })
            .unwrap();

        assert_eq!(code, Some(0));
        assert_eq!(lines, ["alpha", "beta", "gamma"]);
        match seen.recv().unwrap() {
            ControlRequest::GitStream { id, cwd, args } => {
                assert_eq!(id, 1, "the client picks the id, starting at 1");
                assert_eq!(cwd, "/repo");
                assert_eq!(args, ["diff"]);
            }
            other => panic!("expected a GitStream request, got {other:?}"),
        }
    }

    fn host_with_peer_streaming() -> (Arc<RemoteHost>, mpsc::Receiver<ControlRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen_tx, seen_rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            match ControlClientMsg::read(&mut sock).unwrap() {
                ControlClientMsg::Hello(_) => {}
                other => panic!("expected Hello, got {other:?}"),
            }
            ControlServerMsg::HelloOk(hello_ok('/'))
                .encode(&mut sock)
                .unwrap();
            sock.flush().unwrap();
            loop {
                let (req_id, req) = match ControlClientMsg::read(&mut sock) {
                    Ok(ControlClientMsg::Request { req_id, req }) => (req_id, req),
                    _ => return,
                };
                let id = match &req {
                    ControlRequest::GitStream { id, .. } => *id,
                    other => panic!("expected GitStream, got {other:?}"),
                };
                seen_tx.send(req).unwrap();
                ControlServerMsg::Response {
                    req_id,
                    reply: ControlReply::Ok(ReplyOk::Unit),
                }
                .encode(&mut sock)
                .unwrap();
                for bytes in [b"alpha\nbe".to_vec(), b"ta\ngamma\n".to_vec()] {
                    ControlServerMsg::Event(ControlEvent::GitChunk { id, bytes })
                        .encode(&mut sock)
                        .unwrap();
                }
                ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code: Some(0),
                    failed: false,
                })
                .encode(&mut sock)
                .unwrap();
                sock.flush().unwrap();
            }
        });

        let sock = TcpStream::connect(addr).unwrap();
        let host = RemoteHost::over_tcp(
            sock,
            "ssh-alias:testbox",
            &ControlHello::host_rpc("tok", "laptop"),
        )
        .unwrap();
        (host, seen_rx)
    }

    fn scripted(
        steps: Vec<Result<GitStreamMsg, mpsc::RecvTimeoutError>>,
    ) -> impl FnMut(Duration) -> Result<GitStreamMsg, mpsc::RecvTimeoutError> {
        let mut steps = steps.into_iter();
        move |_| {
            steps
                .next()
                .unwrap_or(Err(mpsc::RecvTimeoutError::Disconnected))
        }
    }

    #[test]
    fn a_stream_that_goes_silent_times_out() {
        let mut lines = Vec::new();
        let queued = AtomicUsize::new(b"alpha\n".len());
        let mut next = scripted(vec![
            Ok(GitStreamMsg::Chunk(b"alpha\n".to_vec())),
            Err(mpsc::RecvTimeoutError::Timeout),
        ]);
        let got = drain_git_stream(&mut next, &queued, Duration::from_millis(120), &mut |l| {
            lines.push(l.to_string())
        });

        let err = got.expect_err("a stream that never ends must not read as success");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert_eq!(lines, ["alpha"], "what did arrive was still delivered");
    }

    #[test]
    fn the_idle_window_is_the_gap_between_messages_not_the_whole_stream() {
        let mut steps: Vec<Result<GitStreamMsg, mpsc::RecvTimeoutError>> = (0..5)
            .map(|i| Ok(GitStreamMsg::Chunk(format!("line {i}\n").into_bytes())))
            .collect();
        steps.push(Ok(GitStreamMsg::End {
            code: Some(0),
            failed: false,
        }));

        let mut lines = Vec::new();
        let queued = AtomicUsize::new(0);
        let mut next = scripted(steps);
        let code = drain_git_stream(&mut next, &queued, Duration::from_millis(150), &mut |l| {
            lines.push(l.to_string())
        })
        .unwrap();

        assert_eq!(code, Some(0));
        assert_eq!(
            lines.len(),
            5,
            "every message reset the window, so none of them was cut: {lines:?}"
        );
    }

    #[test]
    fn a_stream_that_outruns_its_queue_budget_is_cut_loose() {
        let registry = GitStreamRegistry::default();
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        registry.insert(
            1,
            StreamSink {
                tx,
                queued: Arc::clone(&queued),
            },
        );

        let chunk = vec![b'x'; 1024 * 1024];
        let pushes = GIT_STREAM_QUEUE_BUDGET / chunk.len() + 2;
        for _ in 0..pushes {
            registry.dispatch(ControlEvent::GitChunk {
                id: 1,
                bytes: chunk.clone(),
            });
        }
        assert!(
            queued.load(Ordering::Acquire) <= GIT_STREAM_QUEUE_BUDGET + chunk.len(),
            "the arrears stopped growing at the budget, not at the diff's size"
        );

        registry.dispatch(ControlEvent::GitChunk {
            id: 1,
            bytes: chunk.clone(),
        });

        let mut lines = Vec::new();
        let err = drain_git_stream(
            &mut |d| rx.recv_timeout(d),
            &queued,
            Duration::from_secs(5),
            &mut |l| lines.push(l.to_string()),
        )
        .expect_err("a cut-off stream must not read as a complete diff");
        assert!(err.to_string().contains("outran"), "{err}");
    }

    #[test]
    fn a_large_but_drained_stream_never_trips_the_budget() {
        let registry = Arc::new(GitStreamRegistry::default());
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        registry.insert(
            1,
            StreamSink {
                tx,
                queued: Arc::clone(&queued),
            },
        );

        let feeder = Arc::clone(&registry);
        let feeder_queued = Arc::clone(&queued);
        let chunks = GIT_STREAM_QUEUE_BUDGET / (1024 * 1024) * 2;
        thread::spawn(move || {
            for _ in 0..chunks {
                let deadline = Instant::now() + Duration::from_secs(10);
                while feeder_queued.load(Ordering::Acquire) > 4 * 1024 * 1024 {
                    if Instant::now() > deadline {
                        break;
                    }
                    thread::yield_now();
                }
                let mut bytes = vec![b'x'; 1024 * 1024 - 1];
                bytes.push(b'\n');
                feeder.dispatch(ControlEvent::GitChunk { id: 1, bytes });
            }
            feeder.dispatch(ControlEvent::GitEnd {
                id: 1,
                code: Some(0),
                failed: false,
            });
        });

        let mut lines = 0usize;
        let code = drain_git_stream(
            &mut |d| rx.recv_timeout(d),
            &queued,
            Duration::from_secs(10),
            &mut |_| lines += 1,
        )
        .expect("a drained stream is not an overrun, however much crosses it");
        assert_eq!(code, Some(0));
        assert_eq!(lines, chunks, "every line arrived");
        assert_eq!(queued.load(Ordering::Acquire), 0, "the arrears settled");
    }

    #[test]
    fn a_link_that_dies_mid_stream_ends_the_read() {
        let host = host_with_peer_dying_mid_stream();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let got = host.git_lines(Path::new("/repo"), &["diff"], &mut |_| {});
            let _ = done_tx.send(got.is_err());
        });

        match done_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(errored) => assert!(
                errored,
                "a stream cut short must report an error, not a successful read of half a diff"
            ),
            Err(_) => panic!(
                "git_lines never returned: the reader is parked on a stream nothing will close"
            ),
        }
    }

    fn host_with_peer_dying_mid_stream() -> Arc<RemoteHost> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            match ControlClientMsg::read(&mut sock).unwrap() {
                ControlClientMsg::Hello(_) => {}
                other => panic!("expected Hello, got {other:?}"),
            }
            ControlServerMsg::HelloOk(hello_ok('/'))
                .encode(&mut sock)
                .unwrap();
            sock.flush().unwrap();
            let (req_id, id) = match ControlClientMsg::read(&mut sock) {
                Ok(ControlClientMsg::Request {
                    req_id,
                    req: ControlRequest::GitStream { id, .. },
                }) => (req_id, id),
                other => panic!("expected a GitStream request, got {other:?}"),
            };
            ControlServerMsg::Response {
                req_id,
                reply: ControlReply::Ok(ReplyOk::Unit),
            }
            .encode(&mut sock)
            .unwrap();
            ControlServerMsg::Event(ControlEvent::GitChunk {
                id,
                bytes: b"alpha\n".to_vec(),
            })
            .encode(&mut sock)
            .unwrap();
            sock.flush().unwrap();
        });

        let sock = TcpStream::connect(addr).unwrap();
        RemoteHost::over_tcp(
            sock,
            "ssh-alias:testbox",
            &ControlHello::host_rpc("tok", "laptop"),
        )
        .unwrap()
    }

    #[test]
    fn link_rtt_pings_and_reports_what_came_back() {
        let (host, seen) = host_with_peer('/', |req| match req {
            ControlRequest::Ping => Some((ControlReply::Ok(ReplyOk::Pong), vec![])),
            other => panic!("unexpected request {other:?}"),
        });

        assert!(
            host.link_rtt().is_some(),
            "a ping that came back is a measurement"
        );
        assert_eq!(seen.recv().unwrap(), ControlRequest::Ping);
        assert_eq!(
            seen.try_recv().ok(),
            None,
            "one row is worth one round trip and no more"
        );
    }

    /// The latency row must mean the distance to the peer, not how long the
    /// peer spent on whatever was asked of it. Timing every call would put a
    /// `ReadFile` of a large file, or a `Git` that shells out, on that row and
    /// read as a network gone seconds slow.
    #[test]
    fn only_a_ping_is_timed() {
        let (host, _seen) = host_with_peer('/', |req| match req {
            ControlRequest::Ping => Some((ControlReply::Ok(ReplyOk::Pong), vec![])),
            ControlRequest::Exists { .. } => Some((ControlReply::Ok(ReplyOk::Bool(true)), vec![])),
            other => panic!("unexpected request {other:?}"),
        });

        assert_eq!(
            host.client().last_rtt(),
            None,
            "a link nothing has pinged has no measurement to report"
        );
        assert!(host.exists(Path::new("/etc/hosts")));
        assert_eq!(
            host.client().last_rtt(),
            None,
            "an ordinary call measures the peer's work, so it leaves the link's latency alone"
        );
        host.client().ping().unwrap();
        assert!(host.client().last_rtt().is_some());
    }

    /// A link that has just gone down keeps the distance it had while it was
    /// up. Blanking the row on the first failed ping would take the number
    /// away at exactly the moment someone is looking at it to work out why the
    /// pane has stopped responding.
    #[test]
    fn a_link_that_goes_down_keeps_its_last_measurement() {
        let served = std::sync::atomic::AtomicBool::new(true);
        let (host, _seen) = host_with_peer('/', move |req| match req {
            ControlRequest::Ping => match served.swap(false, Ordering::Relaxed) {
                true => Some((ControlReply::Ok(ReplyOk::Pong), vec![])),
                // Hanging up rather than answering, which is what a peer whose
                // machine went away looks like from here.
                false => None,
            },
            other => panic!("unexpected request {other:?}"),
        });

        let measured = host.link_rtt().expect("the first ping came back");
        assert_eq!(host.link_rtt(), Some(measured));
        assert!(!host.is_connected(), "the second ping took the link down");
        assert_eq!(host.link_rtt(), Some(measured));
    }

    #[test]
    fn missing_peer_agent_environment_is_not_the_local_environment() {
        let (host, _) = host_with_peer('/', |_| {
            panic!("environment is hello metadata, not an arbitrary execution request")
        });
        assert_eq!(
            host.agent_environment().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn agent_environment_uses_the_peer_scope_and_excludes_credentials() {
        let (host, _) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::AGENT_ENVIRONMENT],
            |_| None,
        );
        let environment = host.agent_environment().unwrap();
        assert_eq!(
            environment.get("KIMI_CODE_HOME").map(String::as_str),
            Some("/peer/隔离 scope")
        );
        assert_eq!(environment.get("OMP_PROFILE").map(String::as_str), Some(""));
        assert!(!environment.contains_key("ANTHROPIC_API_KEY"));
        assert!(!environment.contains_key("PATH"));
    }

    #[test]
    fn shells_come_from_the_peer() {
        let (host, seen) = host_with_peer('/', |req| match req {
            ControlRequest::Shells => Some((
                ControlReply::Ok(ReplyOk::Shells(crate::core::shells::ShellInventory {
                    shells: vec![crate::core::shells::DetectedShell {
                        label: "zsh".into(),
                        program: "/usr/bin/zsh".into(),
                        args: vec!["--no-rcs".into()],
                        args_are_lyrel_defaults: false,
                        user_authored: false,
                    }],
                    default_name: "zsh".into(),
                })),
                vec![],
            )),
            other => panic!("unexpected request {other:?}"),
        });

        let inv = host.shells().unwrap();
        assert_eq!(seen.recv().unwrap(), ControlRequest::Shells);
        assert_eq!(inv.default_name, "zsh");
        assert_eq!(inv.shells[0].program, "/usr/bin/zsh");
        assert_eq!(inv.shells[0].args, ["--no-rcs"]);
        assert!(!inv.shells[0].args_are_lyrel_defaults);
    }

    #[test]
    fn scoped_launch_uses_the_remote_default_shell_not_the_desktop_platform() {
        let (host, seen) = host_with_peer('/', |req| match req {
            ControlRequest::Shells => Some((
                ControlReply::Ok(ReplyOk::Shells(crate::core::shells::ShellInventory {
                    shells: vec![crate::core::shells::DetectedShell {
                        label: "Remote default".into(),
                        program: "/usr/bin/bash".into(),
                        args: vec![],
                        args_are_lyrel_defaults: true,
                        user_authored: false,
                    }],
                    default_name: "Remote default".into(),
                })),
                vec![],
            )),
            other => panic!("unexpected request {other:?}"),
        });
        let line = "PI_IMAGE_PROTOCOL=none pi --session '/remote/session with spaces.jsonl'";
        assert_eq!(
            crate::host::session_operations::scoped_launch_command(&*host, line, None).unwrap(),
            line
        );
        assert_eq!(seen.recv().unwrap(), ControlRequest::Shells);
    }

    /// A remote host has no in-process answer, so the repo probe asks the
    /// far side's `git` — and has to tell a refusal about the directory from
    /// a `git` that never ran, or a tab loses its group to a flaky link.
    #[test]
    fn a_remote_repo_probe_tells_not_a_repo_from_a_failure() {
        use crate::core::git::{RepoProbe, probe_repo};
        let out = |status: i32, stdout: &str| {
            ControlReply::Ok(ReplyOk::Output(Output {
                status: Some(status),
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            }))
        };
        let (host, _seen) = host_with_peer('/', move |req| {
            let reply = match req {
                ControlRequest::Git { cwd, args } => match (cwd.as_str(), args[0].as_str()) {
                    ("/repo", "rev-parse") => out(0, "/repo\n/repo/.git\n/repo/.git\n"),
                    ("/repo", "symbolic-ref") => out(0, "main\n"),
                    ("/repo", _) => out(0, "3\t1\tsrc/main.rs\n"),
                    ("/plain", _) => out(128, ""),
                    ("/shim", _) => out(1, ""),
                    ("/gone", _) => {
                        ControlReply::Err(WireError::new(WireErrorKind::NotFound, "no cwd"))
                    }
                    _ => ControlReply::Err(WireError::new(WireErrorKind::ConnectionReset, "")),
                },
                ControlRequest::Stat { path } if path == "/gone" => {
                    ControlReply::Err(WireError::new(WireErrorKind::NotFound, "gone"))
                }
                ControlRequest::Stat { .. } => ControlReply::Ok(ReplyOk::Meta(meta())),
                other => panic!("unexpected request {other:?}"),
            };
            Some((reply, vec![]))
        });
        let h: &dyn Host = host.as_ref();

        let RepoProbe::Repo(snap) = probe_repo(h, Path::new("/repo")) else {
            panic!("/repo is a repository");
        };
        assert_eq!(
            (snap.root.as_path(), snap.branch.as_str()),
            (Path::new("/repo"), "main")
        );
        assert_eq!(snap.counts, Some((3, 1)));
        assert_eq!(probe_repo(h, Path::new("/plain")), RepoProbe::NotARepo);
        assert_eq!(probe_repo(h, Path::new("/gone")), RepoProbe::NotARepo);
        assert_eq!(probe_repo(h, Path::new("/shim")), RepoProbe::Failed);
        assert_eq!(probe_repo(h, Path::new("/flaky")), RepoProbe::Failed);
    }

    #[test]
    fn agent_sessions_come_from_the_peer_scoped_to_the_directory() {
        use crate::core::agent_history::PastSession;
        use crate::core::cli_agent::CLIAgent;
        let session = PastSession {
            scope: None,
            agent: CLIAgent::Pi,
            id: "0199-abc".into(),
            cwd: Some(PathBuf::from("/home/me/repo")),
            title: "add a retry".into(),
            branch: Some("main".into()),
            updated: 42,
        };
        let reply = session.clone();
        let (host, seen) = host_with_peer('/', move |req| match req {
            ControlRequest::AgentSessions {
                directory: Some(directory),
            } if directory == "/home/me/repo" => Some((
                ControlReply::Ok(ReplyOk::AgentSessions(vec![reply.clone()])),
                vec![],
            )),
            other => panic!("unexpected request {other:?}"),
        });

        let found = host
            .agent_sessions(Some(Path::new("/home/me/repo")))
            .unwrap();
        assert_eq!(
            seen.recv().unwrap(),
            ControlRequest::AgentSessions {
                directory: Some("/home/me/repo".into())
            }
        );
        assert_eq!(found, vec![session]);

        // No directory: the unscoped request is what an unscoped consumer sends.
        let (host, seen) = host_with_peer('/', |req| match req {
            ControlRequest::AgentSessions { directory: None } => {
                Some((ControlReply::Ok(ReplyOk::AgentSessions(Vec::new())), vec![]))
            }
            other => panic!("unexpected request {other:?}"),
        });
        assert!(host.agent_sessions(None).unwrap().is_empty());
        assert_eq!(
            seen.recv().unwrap(),
            ControlRequest::AgentSessions { directory: None }
        );
    }

    #[test]
    fn path_arithmetic_follows_the_peer_not_the_client() {
        let (host, _seen) =
            host_with_peer('/', |_| Some((ControlReply::Ok(ReplyOk::Unit), vec![])));
        let host: &dyn Host = host.as_ref();

        assert_eq!(host.separator(), '/');
        assert_eq!(
            host.join(Path::new("/home/me"), "src"),
            PathBuf::from("/home/me/src"),
            "joins with the remote's separator on every client platform"
        );
        assert!(
            host.is_absolute(Path::new("/home/me")),
            "a POSIX remote path is absolute even where std::path disagrees"
        );
        assert!(!host.is_absolute(Path::new("home/me")));
        assert!(!host.is_absolute(Path::new("C:/home")));
    }

    #[test]
    fn a_windows_peer_gets_windows_path_semantics() {
        let (host, _seen) =
            host_with_peer('\\', |_| Some((ControlReply::Ok(ReplyOk::Unit), vec![])));
        let host: &dyn Host = host.as_ref();

        assert_eq!(host.separator(), '\\');
        assert_eq!(
            host.join(Path::new("C:\\src"), "main.rs"),
            PathBuf::from("C:\\src\\main.rs")
        );
        assert!(host.is_absolute(Path::new("C:\\src")));
        assert!(host.is_absolute(Path::new("\\\\server\\share")));
        assert!(!host.is_absolute(Path::new("src\\main.rs")));
    }

    #[test]
    fn read_methods_map_to_one_request_each() {
        let (host, seen) = host_with_peer('/', |req| {
            let reply = match req {
                ControlRequest::ReadDir { .. } => ReplyOk::Entries(vec![Entry {
                    name: "src".into(),
                    is_dir: true,
                    is_symlink: false,
                    ignored: false,
                }]),
                ControlRequest::Stat { .. } => ReplyOk::Meta(meta()),
                ControlRequest::Exists { .. } => ReplyOk::Bool(true),
                ControlRequest::Canonicalize { .. } => ReplyOk::Path("/real".into()),
                ControlRequest::RepoRoot { .. } => ReplyOk::OptPath(Some("/repo".into())),
                ControlRequest::Search { .. } => ReplyOk::Hits(vec![]),
                ControlRequest::Git { .. } => ReplyOk::Output(Output {
                    status: Some(0),
                    stdout: b" M src/main.rs\n".to_vec(),
                    stderr: Vec::new(),
                }),
                other => panic!("unexpected request {other:?}"),
            };
            Some((ControlReply::Ok(reply), vec![]))
        });
        let h: &dyn Host = host.as_ref();

        assert_eq!(
            h.read_dir(Path::new("/p"), Some(Path::new("/")))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(h.stat(Path::new("/f")).unwrap(), meta());
        assert!(h.exists(Path::new("/f")));
        assert_eq!(
            h.canonicalize(Path::new("/a/../b")).unwrap(),
            PathBuf::from("/real")
        );
        assert_eq!(
            h.repo_root(Path::new("/p/deep")).unwrap(),
            Some(PathBuf::from("/repo"))
        );
        assert!(
            h.search(&[PathBuf::from("/p")], "q", 10, 100, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            h.git(Path::new("/repo"), &["status"])
                .unwrap()
                .stdout_trimmed(),
            "M src/main.rs"
        );

        let sent: Vec<_> = (0..7).map(|_| seen.recv().unwrap()).collect();
        assert!(matches!(sent[0], ControlRequest::ReadDir { .. }));
        assert!(matches!(sent[1], ControlRequest::Stat { .. }));
        assert!(matches!(sent[2], ControlRequest::Exists { .. }));
        assert!(matches!(sent[3], ControlRequest::Canonicalize { .. }));
        assert!(matches!(sent[4], ControlRequest::RepoRoot { .. }));
        assert!(matches!(sent[5], ControlRequest::Search { .. }));
        assert!(matches!(sent[6], ControlRequest::Git { .. }));
    }

    #[test]
    fn mutations_are_a_single_request_with_no_probe() {
        let (host, seen) = host_with_peer('/', |req| {
            let reply = match req {
                ControlRequest::Rename { .. } => ControlReply::Err(WireError::new(
                    WireErrorKind::AlreadyExists,
                    "/b already exists",
                )),
                _ => ControlReply::Ok(ReplyOk::Unit),
            };
            Some((reply, vec![]))
        });
        let h: &dyn Host = host.as_ref();

        h.create_file_new(Path::new("/new")).unwrap();
        h.create_dir(Path::new("/a/b"), true).unwrap();
        h.remove(Path::new("/gone"), false).unwrap();
        let e = h.rename(Path::new("/a"), Path::new("/b")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(e.to_string().contains("/b already exists"));

        let sent: Vec<_> = (0..4).map(|_| seen.recv().unwrap()).collect();
        assert!(matches!(sent[0], ControlRequest::CreateFileNew { .. }));
        assert!(matches!(
            sent[1],
            ControlRequest::CreateDir {
                recursive: true,
                ..
            }
        ));
        assert!(matches!(sent[2], ControlRequest::Remove { .. }));
        assert!(matches!(sent[3], ControlRequest::Rename { .. }));
        assert_eq!(seen.try_recv().ok(), None, "no probing round trips");
    }

    #[test]
    fn file_contents_ride_the_blob_in_both_directions() {
        let content: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        let served = content.clone();
        let (host, _seen) = host_with_peer('/', move |req| match req {
            ControlRequest::ReadFile { .. } => Some((
                ControlReply::Ok(ReplyOk::FileMeta { meta: meta() }),
                served.clone(),
            )),
            ControlRequest::WriteFile { .. } => {
                Some((ControlReply::Ok(ReplyOk::Meta(meta())), vec![]))
            }
            other => panic!("unexpected request {other:?}"),
        });
        let h: &dyn Host = host.as_ref();

        assert_eq!(h.read_file(Path::new("/f"), u64::MAX).unwrap(), content);
        h.write_file(Path::new("/f"), &content).unwrap();
    }

    #[test]
    fn read_file_over_the_limit_fails_without_transferring() {
        let (host, _seen) = host_with_peer('/', |_| {
            Some((
                ControlReply::Err(WireError::new(
                    WireErrorKind::FileTooLarge,
                    "/big is 900 MB, over the 10 MB limit",
                )),
                vec![],
            ))
        });
        let e = host
            .read_file(Path::new("/big"), 10 * 1024 * 1024)
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::FileTooLarge);
        assert!(e.to_string().contains("900 MB"));
    }

    #[test]
    fn a_bounded_git_blob_is_one_request_and_one_blob_reply() {
        use crate::daemon::control::feature::GIT_BLOB;
        let body = b"fn v1() {}\n".to_vec();
        let served = body.clone();
        let (host, seen) = host_with_peer_announcing('/', &[GIT_BLOB], move |req| match req {
            ControlRequest::GitBlob {
                cwd,
                spec,
                max_bytes,
            } => {
                assert_eq!(cwd, "/repo");
                assert_eq!(spec, ":src/a.rs");
                assert_eq!(*max_bytes, 4096);
                Some((
                    ControlReply::Ok(ReplyOk::GitBlob {
                        present: true,
                        len: served.len() as u64,
                    }),
                    served.clone(),
                ))
            }
            other => panic!("unexpected request {other:?}"),
        });

        let bytes = host
            .git_blob(Path::new("/repo"), ":src/a.rs", 4096)
            .unwrap()
            .expect("the version exists");
        assert_eq!(bytes, body);
        assert!(matches!(
            seen.recv().unwrap(),
            ControlRequest::GitBlob { .. }
        ));
    }

    #[test]
    fn an_absent_git_blob_side_is_none_not_an_error() {
        use crate::daemon::control::feature::GIT_BLOB;
        let (host, _seen) = host_with_peer_announcing('/', &[GIT_BLOB], |req| match req {
            ControlRequest::GitBlob { .. } => Some((
                ControlReply::Ok(ReplyOk::GitBlob {
                    present: false,
                    len: 0,
                }),
                vec![],
            )),
            other => panic!("unexpected request {other:?}"),
        });
        assert_eq!(
            host.git_blob(Path::new("/repo"), "HEAD:missing.rs", 4096)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_peer_without_git_blob_is_never_asked() {
        let (host, seen) = host_with_peer('/', |_| None);
        let error = host
            .git_blob(Path::new("/repo"), ":a.rs", 4096)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported, "{error}");
        assert!(error.to_string().contains("update lyrel-server"), "{error}");
        assert!(
            seen.try_recv().is_err(),
            "a request an older peer cannot decode must never be sent"
        );
    }

    #[test]
    fn a_git_blob_reply_that_breaks_its_own_bound_is_refused() {
        use crate::daemon::control::feature::GIT_BLOB;
        let (host, _seen) = host_with_peer_announcing('/', &[GIT_BLOB], |req| match req {
            ControlRequest::GitBlob { .. } => Some((
                ControlReply::Ok(ReplyOk::GitBlob {
                    present: true,
                    len: 99,
                }),
                b"short".to_vec(),
            )),
            other => panic!("unexpected request {other:?}"),
        });
        let error = host
            .git_blob(Path::new("/repo"), ":a.rs", 4096)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }

    #[test]
    fn a_nonzero_git_exit_is_ok_not_err() {
        let (host, _seen) = host_with_peer('/', |_| {
            Some((
                ControlReply::Ok(ReplyOk::Output(Output {
                    status: Some(128),
                    stdout: Vec::new(),
                    stderr: b"not a git repository".to_vec(),
                })),
                vec![],
            ))
        });
        let out = host.git(Path::new("/tmp"), &["rev-parse"]).unwrap();
        assert_eq!(out.status, Some(128));
        assert!(!out.success());
        assert_eq!(out.stderr_trimmed(), "not a git repository");
    }

    #[test]
    fn the_host_id_is_derived_from_the_connection_and_is_stable() {
        let (a, _sa) = host_with_peer('/', |_| Some((ControlReply::Ok(ReplyOk::Unit), vec![])));
        let (b, _sb) = host_with_peer('/', |_| Some((ControlReply::Ok(ReplyOk::Unit), vec![])));
        assert_eq!(a.id(), b.id(), "same connection key, same id");
        assert_eq!(a.id(), a.id());
        assert_ne!(a.id(), HostId::LOCAL, "a remote host is never the local id");
        assert_eq!(a.id(), HostId::from_connection_key("ssh-alias:testbox"));
    }

    #[test]
    fn watch_events_reach_the_subscription() {
        let (host, seen) = host_with_peer('/', |req| match req {
            ControlRequest::WatchOpen { .. } => {
                Some((ControlReply::Ok(ReplyOk::WatchId(7)), vec![]))
            }
            ControlRequest::WatchSet { .. } | ControlRequest::WatchClose { .. } => {
                Some((ControlReply::Ok(ReplyOk::Unit), vec![]))
            }
            other => panic!("unexpected request {other:?}"),
        });

        let sub = host.watch(&[PathBuf::from("/p")]).unwrap();
        assert!(matches!(
            seen.recv().unwrap(),
            ControlRequest::WatchOpen { .. }
        ));

        host.watches.dispatch(ControlEvent::Watch {
            id: 7,
            paths: vec!["/p/a".into(), "/p/b".into()],
        });
        assert_eq!(
            sub.events().recv_blocking().unwrap(),
            vec![PathBuf::from("/p/a"), PathBuf::from("/p/b")]
        );

        host.watches.dispatch(ControlEvent::Watch {
            id: 999,
            paths: vec!["/elsewhere".into()],
        });
        assert!(sub.events().is_empty());

        sub.set_dirs(&[PathBuf::from("/p"), PathBuf::from("/q")])
            .unwrap();
        assert!(matches!(
            seen.recv().unwrap(),
            ControlRequest::WatchSet { .. }
        ));

        host.watches.dispatch(ControlEvent::WatchOverflow { id: 7 });
        assert_eq!(
            sub.events().recv_blocking().unwrap(),
            vec![PathBuf::from("/p"), PathBuf::from("/q")],
            "overflow must name the updated set, not the one watch() opened with"
        );
    }

    #[test]
    fn dropping_the_subscription_closes_it_on_the_server() {
        let (host, seen) = host_with_peer('/', |req| match req {
            ControlRequest::WatchOpen { .. } => {
                Some((ControlReply::Ok(ReplyOk::WatchId(4)), vec![]))
            }
            _ => Some((ControlReply::Ok(ReplyOk::Unit), vec![])),
        });

        let sub = host.watch(&[PathBuf::from("/p")]).unwrap();
        assert!(matches!(
            seen.recv().unwrap(),
            ControlRequest::WatchOpen { .. }
        ));
        drop(sub);
        match seen.recv().unwrap() {
            ControlRequest::WatchClose { id } => assert_eq!(id, 4),
            other => panic!("expected WatchClose, got {other:?}"),
        }
    }

    #[test]
    fn a_dead_link_reports_disconnected() {
        let (host, _seen) = host_with_peer('/', |_| None);
        let h: &dyn Host = host.as_ref();
        assert!(h.is_connected());
        let e = h.stat(Path::new("/f")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert!(!h.is_connected());
    }

    #[test]
    fn a_reply_of_the_wrong_shape_is_invalid_data() {
        let (host, _seen) =
            host_with_peer('/', |_| Some((ControlReply::Ok(ReplyOk::Pong), vec![])));
        let e = host.stat(Path::new("/f")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("Pong"));
    }

    #[test]
    fn remote_host_is_object_safe() {
        let (host, _seen) =
            host_with_peer('/', |_| Some((ControlReply::Ok(ReplyOk::Unit), vec![])));
        let shared: SharedHost = host.into_shared();
        assert_eq!(shared.separator(), '/');
    }

    #[test]
    fn a_peer_without_read_link_requires_upgrade_without_wire_or_local_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("local-only");
        std::fs::write(&marker, b"must not inspect this machine").unwrap();
        let (host, seen) = host_with_peer('/', |req| {
            panic!("an old peer was sent {req:?}");
        });
        let error = host.read_link(&marker).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("update lyrel-server"));
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(host.is_connected(), "the link survives the refusal");
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"must not inspect this machine"
        );
    }

    #[test]
    fn remote_read_link_preserves_raw_target_and_peer_path_semantics() {
        for (separator, target) in [('/', "../shared"), ('\\', r"..\shared")] {
            let (host, seen) = host_with_peer_announcing(
                separator,
                &[crate::daemon::control::feature::READ_LINK],
                move |_| Some((ControlReply::Ok(ReplyOk::Path(target.into())), vec![])),
            );
            assert_eq!(
                host.read_link(Path::new("private/alias")).unwrap(),
                Path::new(target)
            );
            assert_eq!(
                seen.recv().unwrap(),
                ControlRequest::ReadLink {
                    path: "private/alias".into()
                }
            );
            host.client().close();
        }
    }

    #[test]
    fn remote_read_link_reports_wrong_shape_and_connection_loss_without_fallback() {
        let (host, _seen) =
            host_with_peer_announcing('/', &[crate::daemon::control::feature::READ_LINK], |_| {
                Some((ControlReply::Ok(ReplyOk::Unit), vec![]))
            });
        assert_eq!(
            host.read_link(Path::new("/alias")).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        host.client().close();
        let (host, _seen) =
            host_with_peer_announcing('/', &[crate::daemon::control::feature::READ_LINK], |_| None);
        assert_eq!(
            host.read_link(Path::new("/alias")).unwrap_err().kind(),
            io::ErrorKind::ConnectionReset
        );
        assert!(!host.is_connected());
    }

    #[test]
    fn a_peer_without_content_search_is_never_sent_the_request() {
        let (host, seen) = host_with_peer('/', |req| {
            panic!("an old peer was sent {req:?}");
        });
        let e = host
            .search_content(
                &[PathBuf::from("/p")],
                &crate::host::ContentQuery {
                    pattern: "x".into(),
                    ..Default::default()
                },
                &crate::host::ContentLimits::default(),
            )
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(
            seen.recv_timeout(Duration::from_millis(200)).is_err(),
            "nothing may reach a peer that cannot decode it"
        );
        assert!(host.is_connected(), "the link survives the refusal");
    }

    #[test]
    fn a_peer_with_content_search_gets_one_request_carrying_the_query() {
        let (host, seen) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::CONTENT_SEARCH],
            |_| {
                Some((
                    ControlReply::Ok(ReplyOk::ContentHits(ContentResults {
                        hits: vec![],
                        truncated: true,
                        files_searched: 3,
                    })),
                    vec![],
                ))
            },
        );
        let query = crate::host::ContentQuery {
            pattern: "needle".into(),
            whole_word: true,
            ..Default::default()
        };
        let found = host
            .search_content(
                &[PathBuf::from("/p")],
                &query,
                &crate::host::ContentLimits::default(),
            )
            .unwrap();
        assert!(found.truncated);
        assert_eq!(found.files_searched, 3);
        match seen.recv().unwrap() {
            ControlRequest::SearchContent {
                roots, query: q, ..
            } => {
                assert_eq!(roots, vec!["/p".to_string()]);
                assert_eq!(q, query);
            }
            other => panic!("expected SearchContent, got {other:?}"),
        }
    }

    fn session_request(host: HostId) -> crate::host::session_operations::SessionOperation {
        use crate::host::session_operations::*;
        SessionOperation {
            expected_host: host.0,
            agent: crate::core::cli_agent::CLIAgent::OhMyPi,
            kind: SessionOperationKind::Fork,
            source_id: "same-id-on-two-hosts".into(),
            cwd: PathBuf::from("/remote/project"),
            launch_argv: vec!["omp".into()],
            scope: SessionScope::default(),
        }
    }

    #[test]
    fn session_operations_refuse_an_older_peer_without_dropping_the_link() {
        let (host, seen) = host_with_peer('/', |request| panic!("old peer received {request:?}"));
        let error = host
            .session_operation(&session_request(host.id()))
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(host.is_connected());
    }

    #[test]
    fn a_remote_fork_error_keeps_its_real_target_on_that_remote_host() {
        use crate::host::session_operations::*;
        let (host, seen) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::SESSION_OPERATIONS],
            |request| {
                let ControlRequest::SessionOperation { request } = request else {
                    panic!("unexpected request");
                };
                let prepared = PreparedSession {
                    host_id: 0,
                    agent: request.agent,
                    source_id: request.source_id.clone(),
                    target_id: Some("real-created-target".into()),
                    session_file: Some(PathBuf::from("/remote/store/real-created-target.jsonl")),
                    title: None,
                    cwd: request.cwd.clone(),
                    scope: request.scope.clone(),
                    launch_argv: request.launch_argv.clone(),
                    command: Some("omp --resume real-created-target".into()),
                    created: true,
                };
                Some((
                    ControlReply::Ok(ReplyOk::SessionOperation(Box::new(Err(
                        prepared.opening_failed("native shutdown failed")
                    )))),
                    vec![],
                ))
            },
        );
        let error = host
            .session_operation(&session_request(host.id()))
            .unwrap_err();
        let retained = error.retained.unwrap();
        assert_eq!(retained.target_id.as_deref(), Some("real-created-target"));
        assert_eq!(retained.host_id, host.id().0);
        assert!(retained.created);
        let ControlRequest::SessionOperation { request } = seen.recv().unwrap() else {
            panic!("wrong request");
        };
        assert_eq!(
            request.expected_host, 0,
            "the remote server executes on its own local host, not the GUI's local host"
        );
        assert_eq!(request.cwd, PathBuf::from("/remote/project"));
    }

    #[test]
    fn antigravity_session_operations_refuse_old_peers_before_the_wire() {
        let (host, seen) = host_with_peer('/', |request| panic!("old peer received {request:?}"));
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Antigravity;
        request.kind = crate::host::session_operations::SessionOperationKind::Resume;
        let error = host.session_operation(&request).unwrap_err();
        assert_eq!(
            error.kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(
            error.message.contains("antigravity-v1"),
            "the missing capability is named: {}",
            error.message
        );
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(host.is_connected());
    }

    #[test]
    fn antigravity_session_operation_travels_with_its_own_capability() {
        use crate::host::session_operations::*;
        let (host, seen) = host_with_peer_announcing(
            '/',
            &[
                crate::daemon::control::feature::SESSION_OPERATIONS,
                crate::daemon::control::feature::ANTIGRAVITY,
            ],
            |request| {
                let ControlRequest::SessionOperation { request } = request else {
                    panic!("unexpected request");
                };
                assert_eq!(request.agent, crate::core::cli_agent::CLIAgent::Antigravity);
                assert_eq!(request.expected_host, 0);
                let prepared = PreparedSession {
                    host_id: 0,
                    agent: request.agent,
                    source_id: request.source_id.clone(),
                    target_id: Some(request.source_id.clone()),
                    session_file: Some(PathBuf::from(
                        "/home/me/.gemini/antigravity-cli/brain/remote-conversation/.system_generated/logs/transcript.jsonl",
                    )),
                    title: Some("Remote Antigravity".into()),
                    cwd: request.cwd.clone(),
                    scope: request.scope.clone(),
                    launch_argv: vec!["agy".into()],
                    command: Some(format!("agy --conversation {}", request.source_id)),
                    created: false,
                };
                Some((
                    ControlReply::Ok(ReplyOk::SessionOperation(Box::new(Ok(prepared)))),
                    vec![],
                ))
            },
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Antigravity;
        request.kind = SessionOperationKind::Resume;
        request.source_id = "remote-conversation".into();
        let prepared = host.session_operation(&request).unwrap();
        assert_eq!(prepared.host_id, host.id().0);
        assert_eq!(prepared.target_id.as_deref(), Some("remote-conversation"));
        let ControlRequest::SessionOperation { request } = seen.recv().unwrap() else {
            panic!("wrong request");
        };
        assert_eq!(request.expected_host, 0);
        assert_eq!(request.source_id, "remote-conversation");
    }

    #[test]
    fn antigravity_history_refuses_an_older_peer_without_the_read_only_browser() {
        let (host, seen) = host_with_peer('/', |request| panic!("old peer received {request:?}"));
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Antigravity;
        request
            .scope
            .environment
            .insert("HOME".into(), "/home/me".into());
        let error = crate::host::session_operations::antigravity::sessions(&*host, None, &request)
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(
            error.message.contains("update lyrel-server"),
            "the older endpoint is told to update: {}",
            error.message
        );
        assert!(
            seen.try_recv().is_err(),
            "history must not send a request the peer cannot browse"
        );
        assert!(host.is_connected());
    }

    #[test]
    fn antigravity_history_reads_the_peer_index_through_the_read_only_browser() {
        use crate::host::sqlite::{
            SqliteColumns, SqliteObject, SqliteObjectKind, SqliteOpened, SqlitePage, SqliteQuery,
            SqliteReply, SqliteRequest, SqliteValue,
        };
        fn text(value: &str) -> SqliteValue {
            SqliteValue::Text {
                preview: value.as_bytes().to_vec(),
                total: value.len() as u64,
            }
        }
        let (host, seen) = host_with_peer_announcing(
            '/',
            &[
                crate::daemon::control::feature::SQLITE_BROWSE,
                crate::daemon::control::feature::AGENT_ENVIRONMENT,
            ],
            |request| {
                let ControlRequest::Sqlite { request } = request else {
                    panic!("unexpected request {request:?}");
                };
                let reply = match &**request {
                    SqliteRequest::Open { path } => {
                        assert_eq!(
                            path,
                            "/home/me/.gemini/antigravity-cli/conversation_summaries.db"
                        );
                        SqliteReply::Opened(SqliteOpened {
                            view: 7,
                            objects: vec![
                                SqliteObject {
                                    name: "conversation_summaries".into(),
                                    kind: SqliteObjectKind::Table,
                                    sql: String::new(),
                                },
                                SqliteObject {
                                    name: "search_schema_version".into(),
                                    kind: SqliteObjectKind::Table,
                                    sql: String::new(),
                                },
                            ],
                            journal_mode: "wal".into(),
                            idle_millis: 30_000,
                            max_age_millis: 180_000,
                        })
                    }
                    SqliteRequest::Columns { table, .. }
                        if table.as_str() == "search_schema_version" =>
                    {
                        SqliteReply::Columns(SqliteColumns {
                            names: vec!["version".into()],
                            order: vec!["version".into()],
                        })
                    }
                    SqliteRequest::Columns { table, .. }
                        if table.as_str() == "conversation_summaries" =>
                    {
                        SqliteReply::Columns(SqliteColumns {
                            names: vec![
                                "conversation_id".into(),
                                "title".into(),
                                "preview".into(),
                                "last_modified_time".into(),
                                "workspace_uris".into(),
                                "parent_conversation_id".into(),
                                "nesting_depth".into(),
                                "app_data_dir".into(),
                            ],
                            order: vec!["conversation_id".into()],
                        })
                    }
                    SqliteRequest::Page { query, .. } if query.table == "search_schema_version" => {
                        SqliteReply::Page(SqlitePage {
                            page: 0,
                            start_column: 0,
                            rows: vec![vec![SqliteValue::Integer { value: 3 }]],
                            more: false,
                        })
                    }
                    SqliteRequest::Page { query, .. }
                        if query.table == "conversation_summaries" =>
                    {
                        assert_eq!(query.sort, Some((3, true)));
                        assert!(query.search.is_empty());
                        SqliteReply::Page(SqlitePage {
                            page: 0,
                            start_column: 0,
                            rows: vec![vec![
                                text("remote-conversation"),
                                text("Remote Antigravity"),
                                text("first prompt"),
                                text("2026-11-18 09:32:12.5207842+00:00"),
                                text("[\"file:///home/me/repo\"]"),
                                text(""),
                                SqliteValue::Integer { value: 0 },
                                text("antigravity-cli"),
                            ]],
                            more: false,
                        })
                    }
                    SqliteRequest::Close { .. } => SqliteReply::Closed,
                    other => panic!("unexpected sqlite request {other:?}"),
                };
                Some((ControlReply::Ok(ReplyOk::Sqlite(Box::new(reply))), vec![]))
            },
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Antigravity;
        request
            .scope
            .environment
            .insert("HOME".into(), "/home/me".into());
        let found = crate::host::session_operations::antigravity::sessions(
            &*host,
            Some(Path::new("/home/me/repo")),
            &request,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "remote-conversation");
        assert_eq!(found[0].title, "Remote Antigravity");
        assert_eq!(found[0].cwd.as_deref(), Some(Path::new("/home/me/repo")));
        assert_eq!(
            found[0]
                .scope
                .as_ref()
                .and_then(|scope| scope.session_file.as_deref()),
            Some(Path::new(
                "/home/me/.gemini/antigravity-cli/brain/remote-conversation/.system_generated/logs/transcript.jsonl"
            ))
        );
        let mut opened = false;
        while let Ok(request) = seen.try_recv() {
            if let ControlRequest::Sqlite { request } = request
                && matches!(request.as_ref(), SqliteRequest::Open { .. })
            {
                opened = true;
            }
        }
        assert!(opened, "the index was opened read-only on its owning peer");
    }

    #[test]
    fn opencode_history_refuses_old_peers_without_trying_local_storage() {
        let (host, _) = host_with_peer('/', |_| {
            panic!("old peer must not receive an unknown history operation")
        });
        let error = host
            .opencode_sessions(&session_request(host.id()))
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(host.is_connected());
    }

    #[test]
    fn opencode_statistics_requires_its_independent_remote_capability() {
        let (host, _) = host_with_peer_announcing(
            '/',
            &[
                crate::daemon::control::feature::HERMES_HISTORY,
                crate::daemon::control::feature::SESSION_OPERATIONS,
            ],
            |_| panic!("must not send statistics to an old or Hermes-only peer"),
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::OpenCode;
        let error = host.opencode_statistics(&request).unwrap_err();
        assert_eq!(
            error.kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(host.is_connected());
    }

    #[test]
    fn opencode_statistics_transports_only_the_selected_host_scope() {
        let (host, _) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::OPENCODE_STATISTICS],
            |request| {
                let ControlRequest::OpenCodeStatistics { request } = request else {
                    panic!("wrong request")
                };
                assert_eq!(request.expected_host, 0);
                assert_eq!(
                    request.scope.environment.get("HOME").map(String::as_str),
                    Some("/remote/selected")
                );
                Some((
                    ControlReply::Ok(ReplyOk::OpenCodeStatistics(Box::new(Ok(
                        serde_json::from_value(serde_json::json!({
                            "identity": "remote-service",
                            "sessions": [],
                            "issues": ["native service offline"],
                        }))
                        .unwrap(),
                    )))),
                    vec![],
                ))
            },
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::OpenCode;
        request
            .scope
            .environment
            .insert("HOME".into(), "/remote/selected".into());
        let found = host.opencode_statistics(&request).unwrap();
        assert_eq!(found.identity, "remote-service");
        assert_eq!(found.issues, ["native service offline"]);
    }

    #[test]
    fn remote_opencode_history_preserves_its_host_and_service_scope() {
        use crate::host::session_operations::opencode::*;
        let (host, _) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::SESSION_OPERATIONS],
            |request| {
                let ControlRequest::OpenCodeSessions { request } = request else {
                    panic!("expected native scoped history");
                };
                assert_eq!(request.expected_host, 0);
                let sessions = vec![crate::core::agent_history::PastSession {
                    agent: crate::core::cli_agent::CLIAgent::OpenCode,
                    id: "ses_remote".into(),
                    title: "Remote native session".into(),
                    cwd: Some(PathBuf::from("/remote/project")),
                    branch: None,
                    updated: 1_700_000_000,
                    scope: Some(request.scope.clone()),
                }];
                Some((
                    ControlReply::Ok(ReplyOk::OpenCodeSessions(Box::new(Ok(sessions)))),
                    vec![],
                ))
            },
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::OpenCode;
        request.scope.opencode = Some(OpenCodeScope {
            server: Some(OpenCodeServer {
                pid: 44,
                urls: vec!["http://127.0.0.1:9876/".into()],
            }),
            ..Default::default()
        });
        let found = host.opencode_sessions(&request).unwrap();
        assert_eq!(found[0].id, "ses_remote");
        assert_eq!(
            found[0].cwd.as_deref(),
            Some(std::path::Path::new("/remote/project"))
        );
        assert_eq!(found[0].scope, Some(request.scope));
    }

    #[test]
    fn hermes_history_refuses_old_peers_without_reading_gui_storage() {
        let (host, _) = host_with_peer('/', |_| {
            panic!("old peer must not receive an unknown Hermes history operation")
        });
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Hermes;
        let error = host.hermes_sessions(&request).unwrap_err();
        assert_eq!(
            error.kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(host.is_connected());
    }

    #[test]
    fn remote_hermes_history_keeps_its_profile_and_host() {
        let (host, _) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::HERMES_HISTORY],
            |request| {
                let ControlRequest::HermesSessions { request } = request else {
                    panic!("expected native Hermes history");
                };
                assert_eq!(request.expected_host, 0);
                assert_eq!(request.scope.profile.as_deref(), Some("work"));
                assert_eq!(
                    request
                        .scope
                        .environment
                        .get("HERMES_HOME")
                        .map(String::as_str),
                    Some("/remote/hermes/profiles/work")
                );
                let sessions = vec![crate::core::agent_history::PastSession {
                    agent: crate::core::cli_agent::CLIAgent::Hermes,
                    id: "20261018_remote".into(),
                    title: "远端标题".into(),
                    cwd: Some(PathBuf::from("/remote/project")),
                    branch: Some("work".into()),
                    updated: 1_700_000_000,
                    scope: Some(request.scope.clone()),
                }];
                Some((
                    ControlReply::Ok(ReplyOk::HermesSessions(Box::new(Ok(sessions)))),
                    vec![],
                ))
            },
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Hermes;
        request.scope.profile = Some("work".into());
        request
            .scope
            .environment
            .insert("HERMES_HOME".into(), "/remote/hermes/profiles/work".into());
        let found = host.hermes_sessions(&request).unwrap();
        assert_eq!(found[0].id, "20261018_remote");
        assert_eq!(found[0].title, "远端标题");
        assert_eq!(
            found[0].cwd.as_deref(),
            Some(std::path::Path::new("/remote/project"))
        );
        assert_eq!(found[0].scope, Some(request.scope.clone()));
        request.expected_host = host.id().0.wrapping_add(1);
        assert_eq!(
            host.hermes_sessions(&request).unwrap_err().kind,
            crate::host::session_operations::SessionErrorKind::WrongHost
        );
    }
    #[test]
    fn hermes_statistics_does_not_send_unknown_requests_to_history_only_peer() {
        let (host, _) = host_with_peer_announcing(
            '/',
            &[crate::daemon::control::feature::HERMES_HISTORY],
            |_| panic!("statistics must require its own capability"),
        );
        let mut request = session_request(host.id());
        request.agent = crate::core::cli_agent::CLIAgent::Hermes;
        assert_eq!(
            host.hermes_statistics(&request).unwrap_err().kind,
            crate::host::session_operations::SessionErrorKind::Unsupported
        );
        assert!(host.is_connected());
    }
}
