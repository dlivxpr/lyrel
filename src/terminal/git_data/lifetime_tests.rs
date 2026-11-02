use super::*;
use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};
use lyrel_core::host::{ContentLimits, ContentQuery, ContentResults, Entry, Meta, Output, SearchHit};
use std::io;
use std::sync::{Mutex, mpsc};

struct ReadGate {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

/// Only scheduling changes: status bytes, Git parsing and indexes use the real Host path.
struct GatedHost {
    inner: SharedHost,
    id: HostId,
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl GatedHost {
    fn new(id: HostId) -> Arc<Self> {
        Arc::new(Self {
            inner: lyrel_core::host::local::LocalHost::new(),
            id,
            gate: Mutex::new(None),
        })
    }

    fn pause_status(&self) -> ReadGate {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        *self.gate.lock().unwrap() = Some((entered_tx, release_rx));
        ReadGate { entered, release }
    }
}

impl Host for GatedHost {
    fn id(&self) -> HostId { self.id }
    fn separator(&self) -> char { self.inner.separator() }
    fn is_absolute(&self, p: &Path) -> bool { self.inner.is_absolute(p) }
    fn read_dir(&self, p: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.inner.read_dir(p, root)
    }
    fn stat(&self, p: &Path) -> io::Result<Meta> { self.inner.stat(p) }
    fn read_file(&self, p: &Path, max: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> { self.inner.canonicalize(p) }
    fn search(&self, roots: &[PathBuf], query: &str, limit: usize, max_dirs: usize, hidden: bool) -> io::Result<Vec<SearchHit>> {
        self.inner.search(roots, query, limit, max_dirs, hidden)
    }
    fn search_content(&self, roots: &[PathBuf], query: &ContentQuery, limits: &ContentLimits) -> io::Result<ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> { self.inner.write_file(p, bytes) }
    fn create_file_new(&self, p: &Path) -> io::Result<()> { self.inner.create_file_new(p) }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> { self.inner.create_dir(p, recursive) }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> { self.inner.rename(from, to) }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> { self.inner.remove(p, recursive) }
    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> { self.inner.repo_root(p) }
    fn git(&self, p: &Path, args: &[&str]) -> io::Result<Output> {
        let output = self.inner.git(p, args);
        if args.contains(&"status") {
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(30))
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
            }
        }
        output
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> { self.inner.shells() }
    fn watch(&self, paths: &[PathBuf]) -> io::Result<WatchSub> { self.inner.watch(paths) }
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(temp.path()).unwrap(),
    );
    let output = std::process::Command::new("git").args(["init", "--quiet"]).current_dir(&root).output().unwrap();
    assert!(output.status.success());
    std::fs::write(root.join("before.txt"), "before\n").unwrap();
    (temp, root)
}

fn pump(vcx: &mut VisualTestContext, mut done: impl FnMut(&gpui_kit::App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        vcx.run_until_parked();
        if vcx.update(|_, cx| done(cx)) { return; }
        assert!(Instant::now() < deadline, "SCM consumer did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn subscribe(vcx: &mut VisualTestContext, window: u64, who: ScmWatcher, host: HostId, root: &Path) {
    vcx.update(|_, cx| {
        cx.default_global::<ScmData>().subscriptions().declare(window, who, Some((host, root.to_path_buf())));
    });
}

#[gpui_kit::test]
fn status_and_index_live_until_the_last_window_holder_leaves(cx: &mut TestAppContext) {
    let (_temp, root) = repository();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let host = GatedHost::new(HostId::LOCAL);
    subscribe(&mut vcx, 1001, ScmWatcher::Panel, host.id(), &root);
    subscribe(&mut vcx, 1002, ScmWatcher::FileTree, host.id(), &root);
    subscribe(&mut vcx, 1003, ScmWatcher::Editor, host.id(), &root);
    subscribe(&mut vcx, 1004, ScmWatcher::GitHub, host.id(), &root);
    app.update_in(&mut vcx, |app, _, cx| app.scm_refresh(host.clone(), root.clone(), cx));
    pump(&mut vcx, |cx| status_of(cx, host.id(), &root).is_some());
    let (status, index) = vcx.update(|_, cx| (status_of(cx, host.id(), &root).unwrap(), index_of(cx, host.id(), &root).unwrap()));
    for owner in [1001, 1002, 1003] {
        vcx.update(|_, cx| cx.global_mut::<ScmData>().release_window(owner));
        vcx.update(|_, cx| {
            assert!(Arc::ptr_eq(&status, &status_of(cx, host.id(), &root).unwrap()));
            assert!(Arc::ptr_eq(&index, &index_of(cx, host.id(), &root).unwrap()));
        });
    }
    let status_weak = Arc::downgrade(&status);
    let index_weak = Arc::downgrade(&index);
    drop(status);
    drop(index);
    vcx.update(|_, cx| cx.global_mut::<ScmData>().release_window(1004));
    vcx.update(|_, cx| {
        assert!(status_of(cx, host.id(), &root).is_none());
        assert!(index_of(cx, host.id(), &root).is_none());
        assert!(cx.global::<ScmData>().is_stale(host.id(), &root));
    });
    assert!(status_weak.upgrade().is_none(), "the old status has no remaining owner");
    assert!(index_weak.upgrade().is_none(), "the old decoration index has no remaining owner");

    std::fs::write(root.join("after.txt"), "after\n").unwrap();
    subscribe(&mut vcx, 1005, ScmWatcher::Editor, host.id(), &root);
    app.update_in(&mut vcx, |app, _, cx| app.scm_refresh(host.clone(), root.clone(), cx));
    pump(&mut vcx, |cx| status_of(cx, host.id(), &root).is_some_and(|status| status.total_entries == 2));
    vcx.update(|_, cx| {
        assert_eq!(index_of(cx, host.id(), &root).unwrap().file("after.txt"), Some(crate::core::git::status::DecoStatus::Untracked));
        assert!(!cx.global::<ScmData>().is_stale(host.id(), &root));
    });
}

fn late_read_cannot_release_a_new_read(cx: &mut TestAppContext, clear_host: bool) {
    let (_temp, root) = repository();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let id = HostId::from_connection_key("scm-lifetime-gated");
    let host = GatedHost::new(id);
    vcx.update(|_, cx| { crate::ui::host_registry::HostRegistry::insert(cx, host.clone()); });
    subscribe(&mut vcx, 2001, ScmWatcher::Panel, id, &root);
    let old = host.pause_status();
    app.update_in(&mut vcx, |app, _, cx| app.scm_refresh(host.clone(), root.clone(), cx));
    pump(&mut vcx, |_| old.entered.try_recv().is_ok());
    vcx.update(|_, cx| {
        let data = cx.global_mut::<ScmData>();
        if clear_host { data.clear_host(id); } else { data.release_window(2001); }
    });
    std::fs::write(root.join("after.txt"), "after\n").unwrap();
    subscribe(&mut vcx, 2002, ScmWatcher::Editor, id, &root);
    let fresh = host.pause_status();
    app.update_in(&mut vcx, |app, _, cx| app.scm_refresh(host.clone(), root.clone(), cx));
    pump(&mut vcx, |_| fresh.entered.try_recv().is_ok());

    // Observe the real landing, not just completion of the worker's Git call.
    vcx.run_until_parked();
    let landed = Arc::new(AtomicUsize::new(0));
    let observed = landed.clone();
    let _subscription = vcx.update(|_, cx| cx.observe_global::<ScmData>(move |_| { observed.fetch_add(1, Ordering::SeqCst); }));
    old.release.send(()).unwrap();
    pump(&mut vcx, |_| landed.load(Ordering::SeqCst) > 0);
    vcx.update(|_, cx| {
        assert!(status_of(cx, id, &root).is_none(), "old context must not publish a status");
        assert!(index_of(cx, id, &root).is_none());
        assert!(cx.global::<ScmData>().probes.is_pending(&(id, root.clone())), "old landing must not release the reconnected read");
    });
    fresh.release.send(()).unwrap();
    pump(&mut vcx, |cx| status_of(cx, id, &root).is_some_and(|status| status.total_entries == 2));
    vcx.update(|_, cx| {
        assert_eq!(index_of(cx, id, &root).unwrap().file("after.txt"), Some(crate::core::git::status::DecoStatus::Untracked));
        assert!(!cx.global::<ScmData>().is_stale(id, &root));
    });
}

#[gpui_kit::test]
fn last_holder_release_rejects_late_status_without_finishing_the_reacquired_read(cx: &mut TestAppContext) {
    late_read_cannot_release_a_new_read(cx, false);
}

#[gpui_kit::test]
fn host_clear_rejects_late_status_without_finishing_the_reconnected_read(cx: &mut TestAppContext) {
    late_read_cannot_release_a_new_read(cx, true);
}
