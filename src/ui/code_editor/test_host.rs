use super::*;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, Meta, Output, SearchHit,
};
use std::io;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

/// Real filesystem adapter with a distinct Host identity and controllable
/// read/write scheduling. Only scheduling is substituted; bytes and metadata
/// still go through LocalHost, as in the existing settings Host fixtures.
pub(super) struct FileHost {
    inner: SharedHost,
    id: HostId,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
    pub(super) entered: Arc<AtomicBool>,
    pub(super) writes: std::sync::atomic::AtomicUsize,
    write_gate: Mutex<Option<mpsc::Receiver<()>>>,
}

impl FileHost {
    pub(super) fn new(id: HostId) -> Arc<Self> {
        Arc::new(Self {
            inner: lyrel_core::host::local::LocalHost::new(),
            id,
            gate: Mutex::new(None),
            entered: Arc::new(AtomicBool::new(false)),
            writes: std::sync::atomic::AtomicUsize::new(0),
            write_gate: Mutex::new(None),
        })
    }

    pub(super) fn pause_next_read(&self) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel();
        self.entered.store(false, Ordering::SeqCst);
        *self.gate.lock().unwrap() = Some(rx);
        tx
    }

    pub(super) fn pause_next_write(&self) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel();
        self.entered.store(false, Ordering::SeqCst);
        *self.write_gate.lock().unwrap() = Some(rx);
        tx
    }
}

impl Host for FileHost {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, p: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.inner.read_dir(p, root)
    }
    fn stat(&self, p: &Path) -> io::Result<Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> io::Result<Vec<u8>> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            gate.recv_timeout(Duration::from_secs(10))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        }
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(p)
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
        let gate = self.write_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            gate.recv_timeout(Duration::from_secs(10))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.remove(p, recursive)
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
}
