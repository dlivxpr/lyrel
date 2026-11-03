pub(super) use crate::ui::file_tree::read_probe::ReadKind;
use std::collections::HashMap;
use lyrel_core::host::{Entry, Host, HostId, Meta, Output, SearchHit, SharedHost, WatchSub};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

pub(super) struct ReadGate {
    entered: Arc<AtomicBool>,
    release: Option<mpsc::Sender<()>>,
}

impl ReadGate {
    pub(super) fn entered(&self) -> bool {
        self.entered.load(Ordering::Acquire)
    }

    pub(super) fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

impl Drop for ReadGate {
    fn drop(&mut self) {
        self.release();
    }
}

struct PausedRead {
    kind: ReadKind,
    entered: Arc<AtomicBool>,
    release: mpsc::Receiver<()>,
}

/// Real filesystem namespace adapter: identical wire paths on two Hosts map
/// to distinct temporary directories. No operation reads a desktop alias.
pub(super) struct NamespaceHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
    paused: Mutex<Vec<PausedRead>>,
    reads: Mutex<HashMap<ReadKind, usize>>,
}

impl NamespaceHost {
    pub(super) fn new(id: HostId, root: &Path) -> Arc<Self> {
        Arc::new(Self {
            id,
            root: lyrel_core::core::path_spelling::local_spelling_buf(root.to_path_buf()),
            local: lyrel_core::host::local::LocalHost::new(),
            paused: Mutex::new(Vec::new()),
            reads: Mutex::new(HashMap::new()),
        })
    }
    pub(super) fn pause(&self, kind: ReadKind) -> ReadGate {
        let (release, receiver) = mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        self.paused.lock().unwrap().push(PausedRead {
            kind,
            entered: entered.clone(),
            release: receiver,
        });
        ReadGate {
            entered,
            release: Some(release),
        }
    }

    pub(super) fn read_count(&self, kind: ReadKind) -> usize {
        self.reads.lock().unwrap().get(&kind).copied().unwrap_or(0)
    }

    fn start_read(&self, kind: ReadKind) {
        *self.reads.lock().unwrap().entry(kind).or_default() += 1;
    }

    fn land_after_gate(&self, kind: ReadKind) {
        let paused = {
            let mut reads = self.paused.lock().unwrap();
            reads
                .iter()
                .position(|read| read.kind == kind)
                .map(|at| reads.remove(at))
        };
        if let Some(paused) = paused {
            paused.entered.store(true, Ordering::Release);
            let _ = paused.release.recv();
        }
    }
    fn path(&self, path: &Path) -> PathBuf {
        self.root
            .join(path.to_string_lossy().trim_start_matches('/'))
    }
    fn wire(&self, path: PathBuf) -> PathBuf {
        match path.strip_prefix(&self.root) {
            Ok(relative) => PathBuf::from(format!(
                "/{}",
                relative.to_string_lossy().replace('\\', "/")
            )),
            Err(_) => path,
        }
    }
}

impl Host for NamespaceHost {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        if self.root == Path::new("/") {
            self.local.separator()
        } else {
            '/'
        }
    }
    fn is_absolute(&self, path: &Path) -> bool {
        if self.root == Path::new("/") {
            self.local.is_absolute(path)
        } else {
            path.to_string_lossy().starts_with('/')
        }
    }
    fn read_dir(&self, path: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.start_read(ReadKind::Directory);
        let result = self.local.read_dir(
            &self.path(path),
            root.map(|root| self.path(root)).as_deref(),
        );
        self.land_after_gate(ReadKind::Directory);
        result
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
        let kind = if query.is_empty() {
            ReadKind::Index
        } else {
            ReadKind::Names
        };
        self.start_read(kind);
        let result = self
            .local
            .search(
                &roots.iter().map(|path| self.path(path)).collect::<Vec<_>>(),
                query,
                limit,
                dirs,
                hidden,
            )
            .map(|mut hits| {
                for hit in &mut hits {
                    hit.path = self.wire(hit.path.clone());
                }
                hits
            });
        self.land_after_gate(kind);
        result
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> io::Result<lyrel_core::host::ContentResults> {
        self.start_read(ReadKind::Contents);
        let mut results = self.local.search_content(
            &roots.iter().map(|path| self.path(path)).collect::<Vec<_>>(),
            query,
            limits,
        )?;
        for hit in &mut results.hits {
            hit.path = self.wire(hit.path.clone());
        }
        self.land_after_gate(ReadKind::Contents);
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
        self.start_read(ReadKind::Root);
        let result = self
            .local
            .repo_root(&self.path(path))
            .map(|root| root.map(|root| self.wire(root)));
        self.land_after_gate(ReadKind::Root);
        result
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
