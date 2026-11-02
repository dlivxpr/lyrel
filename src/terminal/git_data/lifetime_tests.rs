use super::*;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Meta, Output, SearchHit,
};
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
        max_dirs: usize,
        hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        self.inner.search(roots, query, limit, max_dirs, hidden)
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
        let output = self.inner.git(p, args);
        if args.contains(&"status") {
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release
                    .recv_timeout(Duration::from_secs(30))
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
            }
        }
        output
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, paths: &[PathBuf]) -> io::Result<WatchSub> {
        self.inner.watch(paths)
    }
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(temp.path()).unwrap(),
    );
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(root.join("before.txt"), "before\n").unwrap();
    (temp, root)
}

fn pump(vcx: &mut VisualTestContext, mut done: impl FnMut(&gpui_kit::App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        vcx.executor().advance_clock(Duration::from_millis(300));
        vcx.run_until_parked();
        if vcx.update(|_, cx| done(cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "SCM consumer did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn subscribe(vcx: &mut VisualTestContext, window: u64, who: ScmWatcher, host: HostId, root: &Path) {
    vcx.update(|_, cx| {
        cx.default_global::<ScmData>().subscriptions().declare(
            window,
            who,
            Some((host, root.to_path_buf())),
        );
    });
}

fn fresh(cx: &gpui_kit::App, host: HostId, root: &Path) -> bool {
    let Some(data) = cx.try_global::<ScmData>() else {
        return false;
    };
    status_of(cx, host, root).is_some()
        && !data.is_stale(host, root)
        && !data.probes.is_pending(&(host, root.to_path_buf()))
        && !data.is_change_pending(host, root)
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
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(host.clone(), root.clone(), cx)
    });
    pump(&mut vcx, |cx| fresh(cx, host.id(), &root));
    let (status, index) = vcx.update(|_, cx| {
        (
            status_of(cx, host.id(), &root).unwrap(),
            index_of(cx, host.id(), &root).unwrap(),
        )
    });
    for owner in [1001, 1002, 1003] {
        vcx.update(|_, cx| cx.global_mut::<ScmData>().release_window(owner));
        vcx.update(|_, cx| {
            assert!(Arc::ptr_eq(
                &status,
                &status_of(cx, host.id(), &root).unwrap()
            ));
            assert!(Arc::ptr_eq(
                &index,
                &index_of(cx, host.id(), &root).unwrap()
            ));
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
    assert!(
        status_weak.upgrade().is_none(),
        "the old status has no remaining owner"
    );
    assert!(
        index_weak.upgrade().is_none(),
        "the old decoration index has no remaining owner"
    );

    std::fs::write(root.join("after.txt"), "after\n").unwrap();
    subscribe(&mut vcx, 1005, ScmWatcher::Editor, host.id(), &root);
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(host.clone(), root.clone(), cx)
    });
    pump(&mut vcx, |cx| {
        fresh(cx, host.id(), &root)
            && status_of(cx, host.id(), &root).is_some_and(|status| status.total_entries == 2)
    });
    vcx.update(|_, cx| {
        assert_eq!(
            index_of(cx, host.id(), &root).unwrap().file("after.txt"),
            Some(crate::core::git::status::DecoStatus::Untracked)
        );
        assert!(!cx.global::<ScmData>().is_stale(host.id(), &root));
    });
}

fn late_read_cannot_release_a_new_read(cx: &mut TestAppContext, clear_host: bool) {
    let (_temp, root) = repository();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let local = GatedHost::new(HostId::LOCAL);
    subscribe(&mut vcx, 2100, ScmWatcher::FileTree, local.id(), &root);
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(local.clone(), root.clone(), cx)
    });
    pump(&mut vcx, |cx| fresh(cx, local.id(), &root));
    let local_snapshot = vcx.update(|_, cx| status_of(cx, local.id(), &root).unwrap());
    let id = HostId::from_connection_key("scm-lifetime-gated");
    let host = GatedHost::new(id);
    vcx.update(|_, cx| {
        crate::ui::host_registry::HostRegistry::insert(cx, host.clone());
    });
    subscribe(&mut vcx, 2001, ScmWatcher::Panel, id, &root);
    let old = host.pause_status();
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(host.clone(), root.clone(), cx)
    });
    pump(&mut vcx, |_| old.entered.try_recv().is_ok());
    vcx.update(|_, cx| {
        let data = cx.global_mut::<ScmData>();
        if clear_host {
            data.clear_host(id);
        } else {
            data.release_window(2001);
        }
    });
    std::fs::write(root.join("after.txt"), "after\n").unwrap();
    subscribe(&mut vcx, 2002, ScmWatcher::Editor, id, &root);
    let fresh_read = host.pause_status();
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(host.clone(), root.clone(), cx)
    });
    pump(&mut vcx, |_| fresh_read.entered.try_recv().is_ok());

    // Observe the real landing, not just completion of the worker's Git call.
    pump(&mut vcx, |cx| {
        let data = cx.global::<ScmData>();
        !data.is_change_pending(id, &root) && data.watches.values().all(|watch| !watch.opening)
    });
    let landed = Arc::new(AtomicUsize::new(0));
    let observed = landed.clone();
    let _subscription = vcx.update(|_, cx| {
        cx.observe_global::<ScmData>(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        })
    });
    old.release.send(()).unwrap();
    pump(&mut vcx, |_| landed.load(Ordering::SeqCst) > 0);
    vcx.update(|_, cx| {
        assert!(
            status_of(cx, id, &root).is_none(),
            "old context must not publish a status"
        );
        assert!(index_of(cx, id, &root).is_none());
        assert!(
            cx.global::<ScmData>()
                .probes
                .is_pending(&(id, root.clone())),
            "old landing must not release the reconnected read"
        );
        assert!(
            Arc::ptr_eq(&local_snapshot, &status_of(cx, local.id(), &root).unwrap()),
            "identical wire paths on another Host remain independent"
        );
    });
    fresh_read.release.send(()).unwrap();
    pump(&mut vcx, |cx| {
        fresh(cx, id, &root)
            && status_of(cx, id, &root).is_some_and(|status| status.total_entries == 2)
    });
    vcx.update(|_, cx| {
        assert_eq!(
            index_of(cx, id, &root).unwrap().file("after.txt"),
            Some(crate::core::git::status::DecoStatus::Untracked)
        );
        assert!(!cx.global::<ScmData>().is_stale(id, &root));
    });
}

#[gpui_kit::test]
fn last_holder_release_rejects_late_status_without_finishing_the_reacquired_read(
    cx: &mut TestAppContext,
) {
    late_read_cannot_release_a_new_read(cx, false);
}

#[gpui_kit::test]
fn host_clear_rejects_late_status_without_finishing_the_reconnected_read(cx: &mut TestAppContext) {
    late_read_cannot_release_a_new_read(cx, true);
}

#[gpui_kit::test]
fn repeated_project_visits_release_old_status_and_decoration_owners(cx: &mut TestAppContext) {
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let host = GatedHost::new(HostId::LOCAL);
    let mut old = Vec::new();
    let mut directories = Vec::new();
    for visit in 0..8 {
        let (temp, root) = repository();
        directories.push(temp);
        subscribe(&mut vcx, 3000 + visit, ScmWatcher::Panel, host.id(), &root);
        app.update_in(&mut vcx, |app, _, cx| {
            app.scm_refresh(host.clone(), root.clone(), cx)
        });
        pump(&mut vcx, |cx| fresh(cx, host.id(), &root));
        old.push(vcx.update(|_, cx| {
            (
                Arc::downgrade(&status_of(cx, host.id(), &root).unwrap()),
                Arc::downgrade(&index_of(cx, host.id(), &root).unwrap()),
            )
        }));
        vcx.update(|_, cx| cx.global_mut::<ScmData>().release_window(3000 + visit));
        vcx.update(|_, cx| {
            assert!(status_of(cx, host.id(), &root).is_none());
            assert!(index_of(cx, host.id(), &root).is_none());
            assert!(
                cx.global::<ScmData>()
                    .known_status(host.id(), &root)
                    .is_none()
            );
        });
        assert!(
            old.iter()
                .all(|(status, index)| status.upgrade().is_none() && index.upgrade().is_none())
        );
    }
}

#[gpui_kit::test]
fn displayed_tree_roots_keep_all_live_indexes_and_release_only_removed_roots(
    cx: &mut TestAppContext,
) {
    let (_first, first) = repository();
    let (_second, second) = repository();
    let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    let host = GatedHost::new(HostId::LOCAL);
    app.update_in(&mut vcx, |app, _, _| app.right_panel_visible = false);
    for root in [&first, &second] {
        subscribe(&mut vcx, 4001, ScmWatcher::Panel, host.id(), root);
        app.update_in(&mut vcx, |app, _, cx| {
            app.scm_refresh(host.clone(), root.clone(), cx)
        });
        pump(&mut vcx, |cx| fresh(cx, host.id(), root));
        discover_repository(&mut vcx, host.as_ref(), root);
        // Separate windows can hold different roots through the same watcher.
        subscribe(
            &mut vcx,
            if root == &first { 4002 } else { 4003 },
            ScmWatcher::Editor,
            host.id(),
            root,
        );
    }
    use crate::core::session::{
        RemoteRef, RemoteTarget, WindowView, WindowViews, WorkspaceId, WorkspaceStore,
    };
    let target = RemoteTarget::Alias {
        alias: "scm-lifetime-tree-host".into(),
    };
    let remote = GatedHost::new(target.host_id());
    let remote_workspace =
        WindowView::on_remote(RemoteRef::new(target.clone(), WorkspaceId::new()));
    let remote_workspace_id = remote_workspace.id;
    app.update_in(&mut vcx, |app, _, cx| {
        let mut local_workspace = WindowView::default();
        local_workspace.id = app.workspace;
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![local_workspace, remote_workspace],
                active: None,
            },
        );
        assert_eq!(app.spawn_host(cx), host.id());
    });
    vcx.update(|_, cx| {
        crate::ui::host_registry::HostRegistry::insert(cx, remote.clone());
    });
    subscribe(&mut vcx, 4004, ScmWatcher::Panel, remote.id(), &second);
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(remote.clone(), second.clone(), cx)
    });
    pump(&mut vcx, |cx| fresh(cx, remote.id(), &second));
    discover_repository(&mut vcx, remote.as_ref(), &second);
    // 4001 moved between projects; the first root is held by 4002.
    app.update_in(&mut vcx, |app, window, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = crate::core::config::RightPanelTab::Files;
        app.tab_code_mut_or_init().unwrap().roots = vec![first.clone(), second.clone()];
        app.scm_sync_watchers(window, cx);
        let data = cx.global_mut::<ScmData>();
        for owner in [4001, 4002, 4003] {
            data.release_window(owner);
        }
        assert!(data.status_for(host.id(), &first).is_some());
        assert!(data.index_for(host.id(), &second).is_some());
        assert!(data.needs_status(host.id(), &first));
        let generation = data.generation(host.id(), &second);

        // Removing one root must not retire the other root's live read identity.
        app.tab_code_mut_or_init().unwrap().roots = vec![second.clone()];
        app.scm_sync_watchers(window, cx);
        let data = cx.global::<ScmData>();
        assert!(data.status_for(host.id(), &first).is_none());
        assert!(data.index_for(host.id(), &first).is_none());
        assert!(data.index_for(host.id(), &second).is_some());
        assert_eq!(data.generation(host.id(), &second), generation);

        // The displayed wire path stays the same while the window changes Host.
        // Real filesystem bytes are shared only by this scheduling fixture;
        // SCM ownership and landing identities must remain Host-specific.
        let local_workspace = app.workspace;
        app.workspace = remote_workspace_id;
        assert_eq!(
            app.spawn_host(cx),
            remote.id(),
            "the fixture must really switch Host authority"
        );
        app.rebind_host(host.id(), cx);
        app.scm_sync_watchers(window, cx);
        cx.global_mut::<ScmData>().release_window(4004);
        assert!(index_of(cx, host.id(), &second).is_none());
        assert!(index_of(cx, remote.id(), &second).is_some());

        app.workspace = local_workspace;
        assert_eq!(app.spawn_host(cx), host.id());
        app.rebind_host(remote.id(), cx);
        app.scm_sync_watchers(window, cx);
        assert!(index_of(cx, remote.id(), &second).is_none());
        assert!(
            status_of(cx, host.id(), &second).is_none(),
            "rebound local root must read fresh"
        );
        assert_ne!(
            cx.global::<ScmData>().generation(host.id(), &second),
            generation
        );

        app.right_panel_visible = false;
        app.scm_sync_watchers(window, cx);
        assert!(index_of(cx, host.id(), &second).is_none());
    });
}

#[gpui_kit::test]
fn clearing_another_host_does_not_discard_a_live_pending_status(cx: &mut TestAppContext) {
    let (_temp, root) = repository();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let local = GatedHost::new(HostId::LOCAL);
    let other = HostId::from_connection_key("scm-unrelated-host");
    subscribe(&mut vcx, 5001, ScmWatcher::Panel, local.id(), &root);
    subscribe(&mut vcx, 5002, ScmWatcher::Panel, other, &root);
    let pending = local.pause_status();
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm_refresh(local.clone(), root.clone(), cx)
    });
    pump(&mut vcx, |_| pending.entered.try_recv().is_ok());
    vcx.update(|_, cx| cx.global_mut::<ScmData>().clear_host(other));
    pending.release.send(()).unwrap();
    pump(&mut vcx, |cx| fresh(cx, local.id(), &root));
    vcx.update(|_, cx| {
        assert_eq!(status_of(cx, local.id(), &root).unwrap().total_entries, 1);
        assert_eq!(
            index_of(cx, local.id(), &root).unwrap().file("before.txt"),
            Some(crate::core::git::status::DecoStatus::Untracked)
        );
        assert!(status_of(cx, other, &root).is_none());
    });
}

#[gpui_kit::test]
fn a_dead_host_is_forgotten_after_its_last_repository_owner_left(cx: &mut TestAppContext) {
    use crate::ui::scm::state::RepoKey;
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let root = PathBuf::from("/same-wire-path");
    let dead = HostId::from_connection_key("scm-retired-host");
    let live = HostId::LOCAL;
    vcx.update(|_, cx| {
        crate::ui::host_registry::HostRegistry::insert(cx, GatedHost::new(dead));
    });
    subscribe(&mut vcx, 8300, ScmWatcher::Panel, dead, &root);
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm.follow_pane(RepoKey {
            host: live,
            root: root.clone(),
        });
        app.scm.select_repository(
            RepoKey {
                host: dead,
                root: root.clone(),
            },
            0,
        );
        for host in [dead, live] {
            app.scm
                .roots
                .insert((host, root.clone()), (Instant::now(), Some(root.clone())));
            app.scm.root_lookups.insert((host, root.clone()));
        }
        cx.global_mut::<ScmData>().release_window(8300);
        assert!(cx.global::<ScmData>().hosts().is_empty());
        crate::ui::host_registry::HostRegistry::remove(cx, dead);
        app.scm_forget_lost_hosts(cx);
        assert!(!app.scm.roots.contains_key(&(dead, root.clone())));
        assert!(!app.scm.root_lookups.contains(&(dead, root.clone())));
        assert!(app.scm.roots.contains_key(&(live, root.clone())));
        assert!(app.scm.root_lookups.contains(&(live, root.clone())));
        assert_eq!(
            app.scm.active_repo().unwrap().host,
            live,
            "the dead override cannot survive until reconnect"
        );
    });
}

fn discover_repository(vcx: &mut VisualTestContext, host: &dyn Host, root: &Path) {
    let probe = std::thread::scope(|scope| {
        scope
            .spawn(|| crate::terminal::git_status::probe_repo(host, root))
            .join()
            .unwrap()
    });
    let crate::terminal::git_status::RepoProbe::Repo(snapshot) = probe else {
        panic!("the real Host must discover this test repository");
    };
    vcx.update(|_, cx| {
        cx.default_global::<crate::terminal::git_status::GitStatusCache>()
            .finish_probe(host.id(), root, Some(snapshot));
    });
}

#[gpui_kit::test]
fn displayed_non_repository_roots_stay_quiet_until_existing_discovery_finds_git(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(dir.path()).unwrap(),
    );
    let host = GatedHost::new(HostId::LOCAL);
    let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| {
        crate::ui::host_registry::HostRegistry::insert(cx, host.clone());
    });
    app.update_in(&mut vcx, |app, _, cx| {
        let leaf = app.tabs[0].pane.first_leaf().unwrap();
        let pane = leaf.terminal().unwrap();
        pane.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(root.clone()));
        });
    });
    for known_negative in [false, true] {
        app.update_in(&mut vcx, |app, window, cx| {
            if known_negative {
                cx.default_global::<crate::terminal::git_status::GitStatusCache>()
                    .finish_probe(host.id(), &root, None);
            }
            app.right_panel_visible = true;
            app.right_panel_tab = crate::core::config::RightPanelTab::Files;
            app.tab_code_mut_or_init().unwrap().roots = vec![root.clone()];
            app.scm_sync_watchers(window, cx);
            assert!(
                cx.try_global::<ScmData>().is_none_or(ScmData::is_quiet),
                "unknown and non-Git display roots must not start recurring SCM Git/RPC work"
            );
        });
    }
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(root.join("new-repository.txt"), "discovered\n").unwrap();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    discover_repository(&mut vcx, host.as_ref(), &nested);
    app.update_in(&mut vcx, |app, window, cx| {
        app.tab_code_mut_or_init().unwrap().roots = vec![root.clone()];
        app.scm_sync_watchers(window, cx);
        assert!(
            cx.try_global::<ScmData>()
                .is_some_and(|data| data.is_subscribed(host.id(), &root)),
            "a root discovered from below must activate its displayed working tree"
        );
    });
    pump(&mut vcx, |cx| fresh(cx, host.id(), &root));
    app.update_in(&mut vcx, |app, window, cx| {
        let previous = index_of(cx, host.id(), &root).unwrap();
        app.tab_code_mut_or_init().unwrap().roots = vec![nested, root.clone(), root.clone()];
        app.scm_sync_watchers(window, cx);
        assert_eq!(
            cx.global::<ScmData>().subs.count(host.id(), &root),
            1,
            "multiple display roots in one working tree share one owner"
        );
        assert!(
            Arc::ptr_eq(&previous, &index_of(cx, host.id(), &root).unwrap()),
            "alias roots do not retire the live decoration snapshot"
        );
    });
    vcx.update(|_, cx| {
        assert_eq!(
            index_of(cx, host.id(), &root)
                .unwrap()
                .file("new-repository.txt"),
            Some(crate::core::git::status::DecoStatus::Untracked)
        );
    });
}

#[gpui_kit::test]
fn a_network_write_finishing_after_the_last_holder_retires_its_idle_counter(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let root = PathBuf::from("/network-retired-root");
    let slot = vcx.update(|_, cx| {
        let data = cx.default_global::<ScmData>();
        data.subscriptions()
            .declare(9200, ScmWatcher::Panel, Some((HostId::LOCAL, root.clone())));
        let slot = data.take_network_slot(HostId::LOCAL, &root).unwrap();
        data.release_window(9200);
        assert_eq!(data.network_slots(HostId::LOCAL, &root), 1);
        slot
    });
    drop(slot);
    app.update_in(&mut vcx, |app, window, cx| {
        app.scm_sync_watchers(window, cx);
        assert!(
            cx.global::<ScmData>().hosts().is_empty(),
            "completed unowned writes must not retain historical Host/root counters"
        );
    });
}
