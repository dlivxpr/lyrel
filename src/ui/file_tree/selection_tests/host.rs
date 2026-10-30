use lyrel_core::host::{Entry, Host, HostId, Meta, Output, SearchHit, SharedHost, WatchSub};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Real filesystem namespace adapter: identical wire paths on two Hosts map
/// to distinct temporary directories. No operation reads a desktop alias.
pub(super) struct NamespaceHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
}

impl NamespaceHost {
    pub(super) fn new(id: HostId, root: &Path) -> Arc<Self> {
        Arc::new(Self {
            id,
            root: root.to_path_buf(),
            local: lyrel_core::host::local::LocalHost::new(),
        })
    }
    fn path(&self, path: &Path) -> PathBuf {
        self.root
            .join(path.to_string_lossy().trim_start_matches('/'))
    }
    fn wire(&self, path: PathBuf) -> PathBuf {
        Path::new("/").join(path.strip_prefix(&self.root).unwrap_or(&path))
    }
}

impl Host for NamespaceHost {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        '/'
    }
    fn is_absolute(&self, path: &Path) -> bool {
        path.to_string_lossy().starts_with('/')
    }
    fn read_dir(&self, path: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.local.read_dir(
            &self.path(path),
            root.map(|root| self.path(root)).as_deref(),
        )
    }
    fn stat(&self, path: &Path) -> io::Result<Meta> {
        self.local.stat(&self.path(path))
    }
    fn read_file(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        self.local.read_file(&self.path(path), max)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.local
            .canonicalize(&self.path(path))
            .map(|path| self.wire(path))
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        dirs: usize,
        hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        self.local.search(
            &roots.iter().map(|path| self.path(path)).collect::<Vec<_>>(),
            query,
            limit,
            dirs,
            hidden,
        )
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> io::Result<lyrel_core::host::ContentResults> {
        let mut results = self.local.search_content(
            &roots.iter().map(|path| self.path(path)).collect::<Vec<_>>(),
            query,
            limits,
        )?;
        for hit in &mut results.hits {
            hit.path = self.wire(hit.path.clone());
        }
        Ok(results)
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<Meta> {
        self.local.write_file(&self.path(path), bytes)
    }
    fn create_file_new(&self, path: &Path) -> io::Result<()> {
        self.local.create_file_new(&self.path(path))
    }
    fn create_dir(&self, path: &Path, recursive: bool) -> io::Result<()> {
        self.local.create_dir(&self.path(path), recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.local.rename(&self.path(from), &self.path(to))
    }
    fn remove(&self, path: &Path, recursive: bool) -> io::Result<()> {
        self.local.remove(&self.path(path), recursive)
    }
    fn repo_root(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        self.local
            .repo_root(&self.path(path))
            .map(|root| root.map(|root| self.wire(root)))
    }
    fn git(&self, path: &Path, args: &[&str]) -> io::Result<Output> {
        self.local.git(&self.path(path), args)
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
        self.local.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
        self.local
            .watch(&dirs.iter().map(|path| self.path(path)).collect::<Vec<_>>())
    }
}
