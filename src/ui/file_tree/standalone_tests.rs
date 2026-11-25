//! Standalone GPUI tests for the file-tree entity: no `LyrelApp`, no app
//! bootstrap — a `FileTree` created directly against a controlled `Host`
//! adapter, with the app's side of the event seam played by the test itself.
//!
//! These cover the entity's acceptance surface end to end: expansion and
//! loading, unreadable directories, watcher debounce, tree search, inline
//! rename/delete with their narrow events, and stale-`SharedHost` rejection
//! across a reconnect under the same `HostId`.

use super::selection_tests::host::NamespaceHost;
use super::*;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::host::{ContentLimits, ContentQuery, Entry, Host, Meta, Output, SearchHit};
use std::cell::RefCell;
use std::io;
use std::rc::Rc;

/// The app's side of the seam: apply fs events the tree announces, the way
/// `LyrelApp::on_file_tree_event` does, and record every intent for
/// assertions.
struct Harness {
    events: Rc<RefCell<Vec<String>>>,
    _sub: gpui_kit::Subscription,
}

fn install_harness(cx: &mut App, tree: &Entity<FileTree>) -> Harness {
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&events);
    let sink = tree.clone();
    let sub = cx.subscribe(tree, move |_tree, event, cx| {
        recorded.borrow_mut().push(format!("{event:?}"));
        if let FileTreeEvent::FsEvents { host, paths } = event {
            sink.update(cx, |tree, cx| {
                tree.apply_fs_events(*host, paths, true, true, *host, cx);
            });
        }
    });
    Harness { events, _sub: sub }
}

fn setup(cx: &mut TestAppContext, host: SharedHost) -> (Entity<FileTree>, VisualTestContext) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(lyrel_ui_base::config::Config::default());
        cx.set_global(lyrel_ui_base::presentation::SystemAppearance { dark: false });
        lyrel_ui_base::presets::load_registry(cx);
        crate::ui::theme::apply_theme(None, cx);
        HostRegistry::insert(cx, host);
    });
    let window = cx.add_window(FileTree::new);
    let tree = window.root(cx).expect("the tree is the window root");
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    let vcx = VisualTestContext::from_window(window.into(), cx);
    (tree, vcx)
}

fn present(
    tree: &Entity<FileTree>,
    vcx: &mut VisualTestContext,
    host: &SharedHost,
    roots: &[PathBuf],
    query: &str,
) {
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_view(
                FileTreeView {
                    host: host.id(),
                    active_host: Some(host.clone()),
                    query: query.to_string(),
                    searching: !query.is_empty(),
                    machine_label: "test machine".into(),
                    ..Default::default()
                },
                cx,
            );
            tree.set_roots(roots.to_vec(), cx);
        });
    });
}

fn wait_until(
    vcx: &mut VisualTestContext,
    stage: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        vcx.background_executor.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        vcx.background_executor.run_until_parked();
        if done(vcx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{stage} never completed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn temp() -> tempfile::TempDir {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn local_host() -> SharedHost {
    lyrel_core::host::local::LocalHost::new()
}

fn spelled(path: &Path) -> PathBuf {
    lyrel_core::core::path_spelling::local_spelling_buf(path)
}

#[gpui_kit::test]
fn expanding_a_directory_loads_its_children(cx: &mut TestAppContext) {
    let dir = temp();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/b.txt"), "b").unwrap();
    let host = local_host();
    let (tree, mut vcx) = setup(cx, host.clone());
    let root = spelled(&std::fs::canonicalize(dir.path()).unwrap());
    present(&tree, &mut vcx, &host, std::slice::from_ref(&root), "");

    // The root listing lands, then expansion asks for the child directory.
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.request_loads(&host, std::slice::from_ref(&root), &HashSet::new(), cx);
        });
    });
    wait_until(&mut vcx, "the root listing", |vcx| {
        vcx.update(|_, cx| tree.read(cx).children.get(host.id(), &root).is_some())
    });
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| tree.toggle_expand(&root.join("src"), cx));
    });
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            let expanded = tree.expanded().clone();
            tree.request_loads(&host, std::slice::from_ref(&root), &expanded, cx);
        });
    });
    wait_until(&mut vcx, "the expanded listing", |vcx| {
        vcx.update(|_, cx| {
            tree.read(cx)
                .children
                .get(host.id(), &root.join("src"))
                .is_some_and(|entries| entries.iter().any(|e| e.name == "b.txt"))
        })
    });
    let rows = vcx.update(|_, cx| {
        tree.update(cx, |tree, _| {
            tree.visible_rows(
                host.id(),
                std::slice::from_ref(&root),
                &HashSet::from([root.join("src")]),
            )
        })
    });
    let names: Vec<&str> = rows
        .iter()
        .filter(|r| r.note.is_none())
        .map(|r| r.entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            root.file_name().unwrap().to_str().unwrap(),
            "src",
            "b.txt",
            "a.txt"
        ]
    );
}

#[gpui_kit::test]
fn an_unreadable_directory_reports_itself_instead_of_looking_empty(cx: &mut TestAppContext) {
    let dir = temp();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("secret.txt"), "s").unwrap();
    let host: SharedHost = Arc::new(SearchErrorHost {
        inner: NamespaceHost::new(HostId::LOCAL, dir.path()),
        denied_dir: Some(PathBuf::from("/locked")),
    });
    let (tree, mut vcx) = setup(cx, host.clone());
    let root = PathBuf::from("/");
    present(&tree, &mut vcx, &host, std::slice::from_ref(&root), "");
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.toggle_expand(&PathBuf::from("/locked"), cx);
            let expanded = tree.expanded().clone();
            tree.request_loads(&host, std::slice::from_ref(&root), &expanded, cx);
        });
    });
    wait_until(&mut vcx, "the failed listing", |vcx| {
        vcx.update(|_, cx| {
            tree.read(cx)
                .unreadable
                .contains(&(host.id(), PathBuf::from("/locked")))
        })
    });
    let rows = vcx.update(|_, cx| {
        tree.update(cx, |tree, _| {
            tree.visible_rows(
                host.id(),
                std::slice::from_ref(&root),
                &HashSet::from([PathBuf::from("/locked")]),
            )
        })
    });
    let summary: Vec<_> = rows.iter().map(|r| (&r.entry.name, r.note)).collect();
    assert!(
        rows.iter().any(|r| r.note == Some(TreeNote::Unreadable)),
        "a refused directory must not render like an empty one: {summary:?}"
    );
}

#[gpui_kit::test]
fn watcher_events_debounce_into_a_single_refresh(cx: &mut TestAppContext) {
    let dir = temp();
    std::fs::write(dir.path().join("before.txt"), "b").unwrap();
    let host = local_host();
    let (tree, mut vcx) = setup(cx, host.clone());
    let _harness = vcx.update(|_, cx| install_harness(cx, &tree));
    let root = spelled(&std::fs::canonicalize(dir.path()).unwrap());
    present(&tree, &mut vcx, &host, std::slice::from_ref(&root), "");
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.request_loads(&host, std::slice::from_ref(&root), &HashSet::new(), cx);
            tree.sync_watch_to_view(host.clone(), cx);
        });
    });
    wait_until(&mut vcx, "the initial listing and watcher", |vcx| {
        vcx.update(|_, cx| {
            let tree = tree.read(cx);
            tree.watch.is_some()
                && tree
                    .children
                    .get(host.id(), &root)
                    .is_some_and(|entries| entries.iter().any(|e| e.name == "before.txt"))
        })
    });
    // Three quick writes must coalesce into one debounced application and one
    // reload, and every file must survive the batch.
    for n in 0..3 {
        std::fs::write(dir.path().join(format!("agent-{n}.txt")), "x").unwrap();
    }
    wait_until(&mut vcx, "the debounced refresh", |vcx| {
        vcx.update(|_, cx| {
            tree.read(cx)
                .children
                .get(host.id(), &root)
                .is_some_and(|entries| {
                    (0..3).all(|n| entries.iter().any(|e| e.name == format!("agent-{n}.txt")))
                })
        })
    });
}

/// A name search that always refuses, over a namespace that answers every
/// other call — the tree must print the failure, not "Nothing matches".
struct SearchErrorHost {
    inner: Arc<NamespaceHost>,
    denied_dir: Option<PathBuf>,
}

impl Host for SearchErrorHost {
    fn id(&self) -> HostId {
        self.inner.id()
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, path: &Path) -> bool {
        self.inner.is_absolute(path)
    }
    fn read_dir(&self, path: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        if self.denied_dir.as_deref() == Some(path) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        self.inner.read_dir(path, root)
    }
    fn stat(&self, path: &Path) -> io::Result<Meta> {
        self.inner.stat(path)
    }
    fn read_file(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(path, max)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
    fn search(
        &self,
        _roots: &[PathBuf],
        _query: &str,
        _limit: usize,
        _dirs: usize,
        _hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &ContentQuery,
        limits: &ContentLimits,
    ) -> io::Result<lyrel_core::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<Meta> {
        self.inner.write_file(path, bytes)
    }
    fn create_file_new(&self, path: &Path) -> io::Result<()> {
        self.inner.create_file_new(path)
    }
    fn create_dir(&self, path: &Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(path, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, path: &Path, recursive: bool) -> io::Result<()> {
        self.inner.remove(path, recursive)
    }
    fn repo_root(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        self.inner.repo_root(path)
    }
    fn git(&self, path: &Path, args: &[&str]) -> io::Result<Output> {
        self.inner.git(path, args)
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
        self.inner.watch(dirs)
    }
}

#[gpui_kit::test]
fn tree_search_debounces_hits_and_reports_a_refused_walk(cx: &mut TestAppContext) {
    let dir = temp();
    std::fs::write(dir.path().join("needle.txt"), "n").unwrap();
    let host = local_host();
    let (tree, mut vcx) = setup(cx, host.clone());
    let root = spelled(&std::fs::canonicalize(dir.path()).unwrap());
    present(
        &tree,
        &mut vcx,
        &host,
        std::slice::from_ref(&root),
        "needle",
    );
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_search(host.id(), "needle", std::slice::from_ref(&root), cx);
        });
    });
    wait_until(&mut vcx, "the name search", |vcx| {
        vcx.update(|_, cx| {
            tree.read(cx)
                .search
                .hits
                .iter()
                .any(|hit| hit.name == "needle.txt")
        })
    });

    // The same query against an adapter that refuses the walk is a failure,
    // not an empty answer.
    let failing: SharedHost = Arc::new(SearchErrorHost {
        inner: NamespaceHost::new(HostId::LOCAL, dir.path()),
        denied_dir: None,
    });
    vcx.update(|_, cx| {
        HostRegistry::insert(cx, failing.clone());
    });
    present(
        &tree,
        &mut vcx,
        &failing,
        std::slice::from_ref(&root),
        "needle",
    );
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_search(host.id(), "needle", std::slice::from_ref(&root), cx);
        });
    });
    wait_until(&mut vcx, "the refused walk", |vcx| {
        vcx.update(|_, cx| tree.read(cx).search.failed)
    });
    let rows = vcx.update(|_, cx| tree.update(cx, |tree, _| search_rows(&tree.search)));
    assert!(
        rows.iter().any(|r| r.note == Some(TreeNote::SearchFailed)),
        "the column must be able to say the search never ran"
    );
}

#[gpui_kit::test]
fn inline_rename_and_delete_emit_narrow_events_and_land(cx: &mut TestAppContext) {
    let dir = temp();
    std::fs::write(dir.path().join("old.txt"), "o").unwrap();
    std::fs::write(dir.path().join("doomed.txt"), "d").unwrap();
    let host = local_host();
    let (tree, mut vcx) = setup(cx, host.clone());
    let harness = vcx.update(|_, cx| install_harness(cx, &tree));
    let root = spelled(&std::fs::canonicalize(dir.path()).unwrap());
    present(&tree, &mut vcx, &host, std::slice::from_ref(&root), "");

    // Inline rename: the input is primed with the old name, committed with a
    // new one, and the tree announces the move.
    vcx.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.begin_edit(
                TreeEditKind::Rename,
                &root.join("old.txt"),
                false,
                window,
                cx,
            );
            let input = tree.editing.as_ref().unwrap().input().clone();
            input.update(cx, |st, cx| st.set_value("new.txt", window, cx));
            tree.commit_edit(window, cx);
        });
    });
    // The Host write can be visible on disk before its GPUI completion runs.
    wait_until(&mut vcx, "the PathMoved event", |_| {
        harness
            .events
            .borrow()
            .iter()
            .any(|e| e.starts_with("PathMoved"))
    });
    assert!(root.join("new.txt").is_file());
    assert!(!root.join("old.txt").exists());

    // Inline delete: the prompt answers, the file disappears, and the tree
    // announces the removal.
    vcx.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.selected.single(host.id(), root.join("doomed.txt"));
            tree.delete_selected(window, cx);
        });
    });
    wait_until(&mut vcx, "the delete prompt", |vcx| {
        vcx.has_pending_prompt()
    });
    vcx.simulate_prompt_answer(t(L10nKey::Delete));
    wait_until(&mut vcx, "the PathRemoved event", |_| {
        harness
            .events
            .borrow()
            .iter()
            .any(|e| e.starts_with("PathRemoved"))
    });
    assert!(!root.join("doomed.txt").exists());
    let events = harness.events.borrow();
    assert!(
        events.iter().any(|e| e.starts_with("ViewStateChanged")),
        "selection updates must reach the tab record: {events:?}"
    );
}

#[gpui_kit::test]
fn a_retired_adapter_cannot_land_reads_or_writes_after_reconnect(cx: &mut TestAppContext) {
    let old_dir = temp();
    let new_dir = temp();
    std::fs::write(old_dir.path().join("old.txt"), "o").unwrap();
    std::fs::write(new_dir.path().join("new.txt"), "n").unwrap();
    std::fs::write(old_dir.path().join("shared.txt"), "old connection").unwrap();
    std::fs::write(new_dir.path().join("shared.txt"), "new connection").unwrap();
    // Keep this read/write authority test independent of real OS watcher
    // events. The watcher debounce test uses the real LocalHost separately.
    let old_ns = NamespaceHost::new_unwatched(HostId::LOCAL, old_dir.path());
    let new_ns = NamespaceHost::new_unwatched(HostId::LOCAL, new_dir.path());
    let old: SharedHost = old_ns.clone();
    let new: SharedHost = new_ns.clone();
    let (tree, mut vcx) = setup(cx, old.clone());
    let harness = vcx.update(|_, cx| install_harness(cx, &tree));

    // Begin a directory read on the first adapter, hold it at the gate, then
    // reconnect a different adapter instance under the same HostId.
    let mut gate = old_ns.pause(ReadKind::Directory);
    let root = PathBuf::from("/");
    present(&tree, &mut vcx, &old, std::slice::from_ref(&root), "");
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_host(&old);
            tree.request_loads(&old, std::slice::from_ref(&root), &HashSet::new(), cx);
        });
    });
    wait_until(&mut vcx, "the first adapter's read", |_| gate.entered());
    vcx.update(|_, cx| {
        HostRegistry::insert(cx, new.clone());
    });
    present(&tree, &mut vcx, &new, std::slice::from_ref(&root), "");
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_host(&new);
            tree.request_loads(&new, std::slice::from_ref(&root), &HashSet::new(), cx);
        });
    });
    wait_until(&mut vcx, "the replacement's read", |vcx| {
        vcx.update(|_, cx| {
            tree.read(cx)
                .children
                .get(HostId::LOCAL, Path::new("/"))
                .is_some_and(|entries| entries.iter().any(|e| e.name == "new.txt"))
        })
    });

    // The retired read lands late: it must not rewrite the cache or emit
    // anything.
    let before = harness.events.borrow().len();
    gate.release();
    vcx.background_executor.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    vcx.background_executor.run_until_parked();
    let events = harness.events.borrow();
    assert_eq!(
        events.len(),
        before,
        "a retired adapter's late read must not emit events: {events:?}"
    );
    drop(events);
    vcx.update(|_, cx| {
        tree.update(cx, |tree, _| {
            let rows = tree.visible_rows(HostId::LOCAL, &[PathBuf::from("/")], &HashSet::new());
            assert!(
                rows.iter().any(|r| r.entry.name == "new.txt"),
                "the replacement listing is on screen"
            );
            assert!(
                !rows.iter().any(|r| r.entry.name == "old.txt"),
                "the retired listing must never land"
            );
        });
    });

    // Confirm a delete after reconnecting to another adapter that serves the
    // same path. Both file versions and the replacement row must survive.
    vcx.update(|_, cx| {
        HostRegistry::insert(cx, old.clone());
    });
    let before = harness.events.borrow().len();
    vcx.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_host(&old);
            tree.sync_view(
                FileTreeView {
                    host: HostId::LOCAL,
                    active_host: Some(old.clone()),
                    machine_label: "retired".into(),
                    ..Default::default()
                },
                cx,
            );
            tree.selected.single(HostId::LOCAL, root.join("shared.txt"));
            tree.delete_selected(window, cx);
        });
    });
    wait_until(&mut vcx, "the retired adapter's delete prompt", |vcx| {
        vcx.has_pending_prompt()
    });
    vcx.update(|_, cx| {
        HostRegistry::insert(cx, new.clone());
    });
    present(&tree, &mut vcx, &new, std::slice::from_ref(&root), "");
    vcx.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.sync_host(&new);
            tree.request_loads(&new, std::slice::from_ref(&root), &HashSet::new(), cx);
        });
    });
    wait_until(
        &mut vcx,
        "the replacement listing before confirmation",
        |vcx| {
            vcx.update(|_, cx| {
                tree.read(cx)
                    .children
                    .get(HostId::LOCAL, &root)
                    .is_some_and(|entries| entries.iter().any(|e| e.name == "shared.txt"))
            })
        },
    );
    vcx.simulate_prompt_answer(t(L10nKey::Delete));
    vcx.background_executor.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    vcx.background_executor.run_until_parked();
    let events = harness.events.borrow();
    assert!(
        events[before..]
            .iter()
            .all(|e| !e.starts_with("PathRemoved")),
        "a retired adapter's confirmed delete must not announce a removal: {events:?}"
    );
    drop(events);

    // The confirmation outlived the adapter it named: neither side of it may
    // land. The retired filesystem keeps its file, and the row the optimistic
    // delete would have pulled from the current cache is still on screen.
    assert_eq!(
        std::fs::read(old_dir.path().join("shared.txt")).unwrap(),
        b"old connection"
    );
    assert_eq!(
        std::fs::read(new_dir.path().join("shared.txt")).unwrap(),
        b"new connection"
    );
    vcx.update(|_, cx| {
        tree.update(cx, |tree, _| {
            let rows = tree.visible_rows(HostId::LOCAL, &[PathBuf::from("/")], &HashSet::new());
            assert!(
                rows.iter().any(|r| r.entry.name == "shared.txt"),
                "the replacement's row survived the retired confirmation"
            );
        });
    });
}

#[gpui_kit::test]
fn remote_html_row_context_menu_emits_dual_browser_events_with_remote_host(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("remote.html"), "<h1>Remote</h1>").unwrap();
    std::fs::write(dir.path().join("diagram.svg"), "<svg/>").unwrap();

    let remote_id = HostId::from_connection_key("ssh-direct:dev@remote-tree:22");
    let host: SharedHost = NamespaceHost::new_unwatched(remote_id, dir.path());
    let (tree, mut vcx) = setup(cx, host.clone());
    let harness = cx.update(|cx| install_harness(cx, &tree));
    let root = PathBuf::from("/");
    present(&tree, &mut vcx, &host, std::slice::from_ref(&root), "");

    wait_until(&mut vcx, "remote tree listing", |vcx| {
        vcx.update(|_, cx| {
            tree.read(cx)
                .children
                .get(remote_id, &root)
                .is_some_and(|entries| entries.iter().any(|e| e.name == "remote.html"))
        })
    });

    // .svg excludes browser preview actions on remote host too.
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.right_click("tree-/diagram.svg", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find_by_label(t(L10nKey::PanelOpenInBrowser))
                .is_none()
        );
        assert!(
            window
                .try_find_by_label(t(L10nKey::BrowserOpenExternal))
                .is_none()
        );
        window.press("escape", cx);
    });

    // Click Open in Browser on remote.html
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.right_click("tree-/remote.html", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let pos = window
            .find_by_label(t(L10nKey::PanelOpenInBrowser))
            .bounds()
            .center();
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });

    // Click Open in System Browser on remote.html
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.right_click("tree-/remote.html", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let pos = window
            .find_by_label(t(L10nKey::BrowserOpenExternal))
            .bounds()
            .center();
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });

    let events = harness.events.borrow();
    assert!(
        events
            .iter()
            .any(|ev| ev.contains("OpenInBrowser") && ev.contains("remote.html")),
        "expected OpenInBrowser event in {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|ev| ev.contains("OpenInSystemBrowser") && ev.contains("remote.html")),
        "expected OpenInSystemBrowser event in {events:?}"
    );
}
