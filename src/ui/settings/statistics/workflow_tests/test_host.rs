use super::*;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, Meta, Output, SearchHit, SharedHost,
    WatchSub,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct ScanGate {
    pub(super) entered: smol::channel::Receiver<()>,
    pub(super) release: smol::channel::Sender<()>,
}

pub(super) struct CountingHost {
    inner: SharedHost,
    root: PathBuf,
    canonical_root: PathBuf,
    reads: AtomicUsize,
    listings: AtomicUsize,
    gate: Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>,
}
impl CountingHost {
    pub(super) fn new(root: &Path) -> Arc<Self> {
        let inner = lyrel_core::host::local::LocalHost::new();
        let canonical_root = inner.canonicalize(root).unwrap();
        Arc::new(Self {
            inner,
            root: root.into(),
            canonical_root,
            reads: AtomicUsize::new(0),
            listings: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }
    pub(super) fn block_scan(&self) -> ScanGate {
        let (entered_tx, entered) = smol::channel::bounded(1);
        let (release, release_rx) = smol::channel::bounded(1);
        *self.gate.lock().unwrap() = Some((entered_tx, release_rx));
        ScanGate { entered, release }
    }
    pub(super) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    pub(super) fn listings(&self) -> usize {
        self.listings.load(Ordering::SeqCst)
    }
    fn contains(&self, path: &Path) -> bool {
        // 发现前使用 launcher 的原始路径；扫描使用 Host 规范化后的路径。
        path.starts_with(&self.root) || path.starts_with(&self.canonical_root)
    }
    fn record(&self, path: &Path) {
        if self.contains(path) {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
    }
}
impl Host for CountingHost {
    fn id(&self) -> HostId {
        HostId::LOCAL
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, path: &Path) -> bool {
        self.inner.is_absolute(path)
    }
    fn read_dir(&self, path: &Path, root: Option<&Path>) -> std::io::Result<Vec<Entry>> {
        self.record(path);
        if self.contains(path) {
            self.listings.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.read_dir(path, root)
    }
    fn stat(&self, path: &Path) -> std::io::Result<Meta> {
        self.record(path);
        self.inner.stat(path)
    }
    fn read_file(&self, path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
        self.record(path);
        if self.contains(path)
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send_blocking(()).unwrap();
                release.recv_blocking().unwrap();
            }
        }
        self.inner.read_file(path, max)
    }
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.record(path);
        self.inner.canonicalize(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.read_link(path)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        hidden: bool,
    ) -> std::io::Result<Vec<SearchHit>> {
        self.inner.search(roots, query, limit, max_dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &ContentQuery,
        limits: &ContentLimits,
    ) -> std::io::Result<ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<Meta> {
        self.inner.write_file(path, bytes)
    }
    fn create_file_new(&self, path: &Path) -> std::io::Result<()> {
        self.inner.create_file_new(path)
    }
    fn create_dir(&self, path: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.create_dir(path, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, path: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.remove(path, recursive)
    }
    fn repo_root(&self, path: &Path) -> std::io::Result<Option<PathBuf>> {
        self.inner.repo_root(path)
    }
    fn git(&self, path: &Path, args: &[&str]) -> std::io::Result<Output> {
        self.inner.git(path, args)
    }
    fn shells(&self) -> std::io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn agent_environment(&self) -> std::io::Result<std::collections::BTreeMap<String, String>> {
        self.inner.agent_environment()
    }
    fn watch(&self, paths: &[PathBuf]) -> std::io::Result<WatchSub> {
        self.inner.watch(paths)
    }
}
