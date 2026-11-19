use super::*;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostRead, Meta, Output, SearchHit,
};
use std::io;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

/// Match the path spelling retained by `EditorBuffers::open`, even when the
/// runner's temporary directory uses a symlink, junction or Windows 8.3 name.
pub fn canonical_tempdir() -> tempfile::TempDir {
    let root = std::thread::spawn(|| {
        lyrel_core::host::local::LocalHost::new().canonicalize(&std::env::temp_dir())
    })
    .join()
    .unwrap()
    .unwrap();
    tempfile::tempdir_in(root).unwrap()
}

/// Deterministic pixel noise that does not compress, so a test fixture can be
/// genuinely larger than the old text limit without shipping a blob.
#[cfg(any(test, feature = "test-support"))]
pub fn noise_png(width: u32, height: u32) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut rgba = image::RgbaImage::new(width, height);
    for pixel in rgba.pixels_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bytes = state.to_le_bytes();
        *pixel = image::Rgba([bytes[0], bytes[1], bytes[2], 255]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("noise png encodes");
    out.into_inner()
}

/// Real filesystem adapter with a distinct Host identity and controllable
/// read/write scheduling. Only scheduling is substituted; bytes and metadata
/// still go through LocalHost, as in the existing settings Host fixtures.
pub struct FileHost {
    inner: SharedHost,
    id: HostId,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
    pub entered: Arc<AtomicBool>,
    pub writes: std::sync::atomic::AtomicUsize,
    write_gate: Mutex<Option<mpsc::Receiver<()>>>,
    fail_write: AtomicBool,
    root: Option<PathBuf>,
    /// Held by every opened read session; one `pause_next_segment` gates the
    /// next segment any of them asks for.
    segment_gate: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    /// Segments actually started across all sessions.
    pub segments: Arc<AtomicUsize>,
    /// Versioned read sessions opened through this host.
    pub open_reads: AtomicUsize,
}

impl FileHost {
    pub fn new(id: HostId) -> Arc<Self> {
        Arc::new(Self {
            inner: lyrel_core::host::local::new(),
            id,
            gate: Mutex::new(None),
            entered: Arc::new(AtomicBool::new(false)),
            writes: std::sync::atomic::AtomicUsize::new(0),
            write_gate: Mutex::new(None),
            fail_write: AtomicBool::new(false),
            root: None,
            segment_gate: Arc::new(Mutex::new(None)),
            segments: Arc::new(AtomicUsize::new(0)),
            open_reads: AtomicUsize::new(0),
        })
    }

    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub fn mapped(id: HostId, root: &Path) -> Arc<Self> {
        let mut host = Self::new(id);
        Arc::get_mut(&mut host).unwrap().root = Some(root.to_path_buf());
        host
    }
    fn path(&self, path: &Path) -> PathBuf {
        self.root.as_ref().map_or_else(
            || path.to_path_buf(),
            |root| root.join(path.to_string_lossy().trim_start_matches('/')),
        )
    }
    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub fn fail_next_write(&self) {
        self.fail_write.store(true, Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn pause_next_read(&self) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel();
        self.entered.store(false, Ordering::SeqCst);
        *self.gate.lock().unwrap() = Some(rx);
        tx
    }

    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub fn pause_next_write(&self) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel();
        self.entered.store(false, Ordering::SeqCst);
        *self.write_gate.lock().unwrap() = Some(rx);
        tx
    }

    /// Holds the next segment any read session asks for, so a test can close or
    /// replace the target while a transfer is genuinely in flight.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pause_next_segment(&self) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel();
        self.entered.store(false, Ordering::SeqCst);
        *self.segment_gate.lock().unwrap() = Some(rx);
        tx
    }
}

impl Host for FileHost {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        if self.root.is_some() {
            '/'
        } else {
            self.inner.separator()
        }
    }
    fn is_absolute(&self, p: &Path) -> bool {
        if self.root.is_some() {
            p.to_string_lossy().starts_with('/')
        } else {
            self.inner.is_absolute(p)
        }
    }
    fn read_dir(&self, p: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        let root = root.map(|root| self.path(root));
        self.inner.read_dir(&self.path(p), root.as_deref())
    }
    fn stat(&self, p: &Path) -> io::Result<Meta> {
        self.inner.stat(&self.path(p))
    }
    fn read_file(&self, p: &Path, max: u64) -> io::Result<Vec<u8>> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            gate.recv_timeout(Duration::from_secs(10))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        }
        self.inner.read_file(&self.path(p), max)
    }
    fn open_read(&self, p: &Path) -> io::Result<Box<dyn HostRead>> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            gate.recv_timeout(Duration::from_secs(10))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        }
        let inner = self.inner.open_read(&self.path(p))?;
        self.open_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(GatedRead {
            inner,
            gate: Arc::clone(&self.segment_gate),
            entered: Arc::clone(&self.entered),
            segments: Arc::clone(&self.segments),
        }))
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        if self.root.is_some() {
            self.inner
                .canonicalize(&self.path(p))
                .map(|_| p.to_path_buf())
        } else {
            self.inner.canonicalize(p)
        }
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        dirs: usize,
        hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        self.inner.search(roots, query, limit, dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &ContentQuery,
        limits: &ContentLimits,
    ) -> io::Result<ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
        if self.fail_write.swap(false, Ordering::SeqCst) {
            self.entered.store(true, Ordering::SeqCst);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected Host write failure",
            ));
        }
        let gate = self.write_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            gate.recv_timeout(Duration::from_secs(10))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.write_file(&self.path(p), bytes)
    }
    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.inner.create_file_new(&self.path(p))
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(&self.path(p), recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(&self.path(from), &self.path(to))
    }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.remove(&self.path(p), recursive)
    }
    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, p: &Path, args: &[&str]) -> io::Result<Output> {
        self.inner.git(p, args)
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
        self.inner.watch(dirs)
    }
    fn sqlite_open(
        &self,
        p: &Path,
        cancel: &lyrel_core::host::sqlite::SqliteCancel,
    ) -> io::Result<lyrel_core::host::sqlite::SqliteOpened> {
        self.inner.sqlite_open(&self.path(p), cancel)
    }
    fn sqlite_columns(
        &self,
        view: u64,
        table: &str,
        cancel: &lyrel_core::host::sqlite::SqliteCancel,
    ) -> io::Result<lyrel_core::host::sqlite::SqliteColumns> {
        self.inner.sqlite_columns(view, table, cancel)
    }
    fn sqlite_count(
        &self,
        view: u64,
        query: &lyrel_core::host::sqlite::SqliteQuery,
        cancel: &lyrel_core::host::sqlite::SqliteCancel,
    ) -> io::Result<u64> {
        self.inner.sqlite_count(view, query, cancel)
    }
    fn sqlite_page(
        &self,
        view: u64,
        query: &lyrel_core::host::sqlite::SqliteQuery,
        cancel: &lyrel_core::host::sqlite::SqliteCancel,
    ) -> io::Result<lyrel_core::host::sqlite::SqlitePage> {
        self.inner.sqlite_page(view, query, cancel)
    }
    fn sqlite_value(
        &self,
        view: u64,
        query: &lyrel_core::host::sqlite::SqliteQuery,
        cell: lyrel_core::host::sqlite::SqliteCellRead,
        cancel: &lyrel_core::host::sqlite::SqliteCancel,
    ) -> io::Result<lyrel_core::host::sqlite::SqliteValue> {
        self.inner.sqlite_value(view, query, cell, cancel)
    }
    fn sqlite_close(&self, view: u64) {
        self.inner.sqlite_close(view)
    }
}

/// A real local session with one controllable pause before the next segment.
struct GatedRead {
    inner: Box<dyn HostRead>,
    gate: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    entered: Arc<AtomicBool>,
    segments: Arc<AtomicUsize>,
}

impl HostRead for GatedRead {
    fn meta(&self) -> Meta {
        self.inner.meta()
    }

    fn read_segment(&mut self, max_bytes: u64) -> io::Result<Vec<u8>> {
        self.segments.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            gate.recv_timeout(Duration::from_secs(10))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        }
        self.inner.read_segment(max_bytes)
    }
}
