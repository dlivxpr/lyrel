//! One slow Host fixture shared by request consumers on TCP and Unix sockets.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::host::{Entry, Host, HostId, Meta, Output, SearchHit, SharedHost, WatchSub};
use crate::host::local::LocalHost;

pub(super) struct SlowGit {
    inner: SharedHost,
    delay: Duration,
    running: Arc<AtomicBool>,
    gate: Option<Gate>,
}

impl Host for SlowGit {
    fn id(&self) -> HostId {
        self.inner.id()
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.inner.read_dir(dir, root)
    }
    fn stat(&self, p: &Path) -> io::Result<Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(p, max_bytes)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        show_hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        self.inner
            .search(roots, query, limit, max_dirs, show_hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &crate::host::ContentQuery,
        limits: &crate::host::ContentLimits,
    ) -> io::Result<crate::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
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
    fn git(&self, _cwd: &Path, _args: &[&str]) -> io::Result<Output> {
        self.running.store(true, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.started.send(()).map_err(io::Error::other)?;
            gate.release.lock().unwrap().recv_timeout(Duration::from_secs(5)).map_err(io::Error::other)?;
            gate.finished.send(()).map_err(io::Error::other)?;
        } else {
            std::thread::sleep(self.delay);
        }
        Ok(Output {
            status: Some(0),
            stdout: b"slow".to_vec(),
            stderr: Vec::new(),
        })
    }
    fn shells(&self) -> io::Result<crate::host::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
        self.inner.watch(dirs)
    }
}

struct Gate {
    started: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    finished: mpsc::Sender<()>,
}

impl SlowGit {
    #[cfg(unix)]
    pub(super) fn delayed(delay: Duration, running: Arc<AtomicBool>) -> SharedHost {
        Arc::new(Self { inner: LocalHost::new(), delay, running, gate: None })
    }

    pub(super) fn gated() -> (SharedHost, mpsc::Receiver<()>, mpsc::Sender<()>, mpsc::Receiver<()>) {
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        let host = Arc::new(Self {
            inner: LocalHost::new(),
            delay: Duration::ZERO,
            running: Arc::new(AtomicBool::new(false)),
            gate: Some(Gate { started: started_tx, release: Mutex::new(release_rx), finished: finished_tx }),
        });
        (host, started, release, finished)
    }
}
