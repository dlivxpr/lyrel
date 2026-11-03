//! Real Host reads land through HostOps into the production workbench. Gates
//! stop a completed backing read before its UI callback, not an executor queue.
use super::host::{NamespaceHost, ReadKind};
use super::*;
use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WorkspaceId, WorkspaceStore};
use crate::terminal::PaneWorkspace;
use crate::ui::app::test_window::harness_with_pane;
use crate::ui::search::files::FileList;
use crate::ui::file_tree::read_probe::{InstanceTicket, Landing};
use std::time::{Duration, Instant};

fn backing(name: &str) -> tempfile::TempDir {
    let temp = temp();
    std::fs::create_dir(temp.path().join("repo")).unwrap();
    std::fs::write(
        temp.path().join("repo").join(name),
        "needle in real contents\n",
    )
    .unwrap();
    temp
}

fn window_at_repo(
    cx: &mut TestAppContext,
) -> (
    Entity<LyrelApp>,
    VisualTestContext,
    crate::daemon::transport::Stream,
) {
    let (app, vcx, mut stream) = harness_with_pane(cx);
    // Advancing the poll clock can replace the test-only Git cwd. Keep the
    // pane's actual cwd rooted through the same message a real daemon sends.
    crate::daemon::protocol::DaemonMsg::Cwd(PathBuf::from("/repo"))
        .encode(&mut stream)
        .expect("the consumer pane receives its cwd");
    (app, vcx, stream)
}

struct Replacement {
    old_dir: tempfile::TempDir,
    new_dir: tempfile::TempDir,
    old: Arc<NamespaceHost>,
    new: Arc<NamespaceHost>,
}

impl Replacement {
    fn new(old_name: &str, new_name: &str, new_id: HostId) -> Self {
        let old_dir = backing(old_name);
        let new_dir = backing(new_name);
        let old = NamespaceHost::new(HostId::LOCAL, old_dir.path());
        let new = NamespaceHost::new(new_id, new_dir.path());
        Self {
            old_dir,
            new_dir,
            old,
            new,
        }
    }

    fn window(
        &self,
        cx: &mut TestAppContext,
    ) -> (
        Entity<LyrelApp>,
        VisualTestContext,
        crate::daemon::transport::Stream,
    ) {
        let (app, mut vcx, stream) = window_at_repo(cx);
        activate(&app, &mut vcx, self.old.clone());
        (app, vcx, stream)
    }
}

fn checkpoint(app: &Entity<LyrelApp>, vcx: &VisualTestContext, kind: ReadKind) -> Option<Landing> {
    app.read_with(vcx, |app, _| app.file_tree.read_probe.checkpoint(kind))
}

fn landed_since(
    app: &Entity<LyrelApp>,
    vcx: &VisualTestContext,
    kind: ReadKind,
    host: InstanceTicket,
    before: Option<Landing>,
) -> bool {
    app.read_with(vcx, |app, _| {
        app.file_tree.read_probe.landed_since(kind, host, before)
    })
}

fn other_target() -> RemoteTarget {
    RemoteTarget::Alias {
        alias: "instance-fence-other-host".into(),
    }
}

fn activate(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, host: SharedHost) {
    app.update_in(vcx, |app, _, cx| {
        let pane = app.tabs[0]
            .pane
            .first_leaf()
            .unwrap()
            .terminal()
            .unwrap()
            .clone();
        if !host.id().is_local() {
            let target = other_target();
            assert_eq!(target.host_id(), host.id());
            let workspace = WorkspaceId::new();
            let view = WindowView::on_remote(RemoteRef::new(target.clone(), workspace));
            app.workspace = view.id;
            let mut views = WorkspaceStore::all(cx).clone();
            views.views.push(view);
            WorkspaceStore::install_for_test(cx, views);
            pane.update(cx, |pane, _| {
                pane.set_workspace(Some(PaneWorkspace {
                    workspace,
                    target,
                    spec: None,
                    label: None,
                    resize_echo: false,
                }))
            });
        }
        HostRegistry::insert(cx, host);
        pane.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(PathBuf::from("/repo")))
        });
        app.set_right_panel_tab(RightPanelTab::Files, cx);
        cx.notify();
    });
}

#[track_caller]
fn until(
    vcx: &mut VisualTestContext,
    stage: &str,
    mut ready: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        vcx.executor().advance_clock(Duration::from_millis(300));
        vcx.background_executor.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        if ready(vcx) {
            return;
        }
        assert!(Instant::now() < deadline, "{stage} never completed");
        std::thread::yield_now();
    }
}

fn listed(app: &Entity<LyrelApp>, vcx: &VisualTestContext, id: HostId, name: &str) -> bool {
    app.read_with(vcx, |app, _| {
        app.file_tree
            .children
            .get(id, Path::new("/repo"))
            .is_some_and(|entries| entries.iter().any(|entry| entry.name == name))
            && !app
                .file_tree
                .loads
                .is_pending(&(id, PathBuf::from("/repo")))
    })
}

fn assert_rows(vcx: &mut VisualTestContext, present: &str, absent: &str) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(id(Path::new(&format!("/repo/{present}"))))
                .visible()
        );
        assert!(
            window
                .try_find(id(Path::new(&format!("/repo/{absent}"))))
                .is_none()
        );
    });
}

#[gpui_kit::test]
fn host_instance_quick_open_without_cwd_shows_no_roots(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = backing("must-not-walk.txt");
    let host = NamespaceHost::new(HostId::LOCAL, root.path());
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        HostRegistry::insert(cx, host.clone());
        assert!(app.file_project_cwds(cx).is_empty());
        assert!(!app.file_tree.host_matches(&(host.clone() as SharedHost)));
        app.quick_open_file(window, cx);
    });
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        app.search.view().unwrap().read(cx).assert_no_file_roots(cx);
    });
    assert_eq!(
        host.read_count(ReadKind::Index),
        0,
        "no roots must not dispatch a walk"
    );
}

fn search_file_rows(app: &Entity<LyrelApp>, vcx: &VisualTestContext) -> Vec<PathBuf> {
    app.read_with(vcx, |app, cx| {
        app.search.view().unwrap().read(cx).file_rows(cx)
    })
}

#[gpui_kit::test]
fn host_instance_all_search_reconnect_replaces_landed_file_rows(cx: &mut TestAppContext) {
    let _serial = serial();
    let fixture = Replacement::new("needle-old.txt", "needle-new.txt", HostId::LOCAL);
    // Both namespaces own the exact same wire path, with different contents.
    std::fs::write(
        fixture.old_dir.path().join("repo/needle-shared.txt"),
        "retired adapter\n",
    )
    .unwrap();
    std::fs::write(
        fixture.new_dir.path().join("repo/needle-shared.txt"),
        "current adapter\n",
    )
    .unwrap();
    let (app, mut vcx, _stream) = fixture.window(cx);
    until(&mut vcx, "initial listing", |vcx| {
        listed(&app, vcx, HostId::LOCAL, "needle-old.txt")
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(crate::ui::search::SearchTab::All, "needle", window, cx)
    });
    until(&mut vcx, "landed old All file source", |vcx| {
        search_file_rows(&app, vcx).contains(&PathBuf::from("/repo/needle-old.txt"))
    });
    let mut gate = fixture.new.pause(ReadKind::Index);
    activate(&app, &mut vcx, fixture.new.clone());
    app.update_in(&mut vcx, |app, window, cx| {
        app.file_tree_host_rebound(window, cx)
    });
    // Check the open consumer immediately, before the replacement can land.
    assert!(
        search_file_rows(&app, &vcx).is_empty(),
        "All retained retired file rows"
    );
    until(&mut vcx, "replacement All walk", |_| gate.entered());
    gate.release();
    until(&mut vcx, "current All file source", |vcx| {
        let rows = search_file_rows(&app, vcx);
        rows.contains(&PathBuf::from("/repo/needle-new.txt"))
            && !rows.contains(&PathBuf::from("/repo/needle-old.txt"))
    });
    // Narrow the same All consumer with real input, then confirm its file row.
    vcx.update(|window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.input("needle-shared.txt", cx);
    });
    until(&mut vcx, "shared path row", |vcx| {
        search_file_rows(&app, vcx) == [PathBuf::from("/repo/needle-shared.txt")]
    });
    vcx.update(|window, cx| window.press("enter", cx));
    until(&mut vcx, "current adapter file opens", |vcx| {
        app.read_with(vcx, |app, cx| {
            app.editor_active_local_file().is_some_and(|file| {
                file.path == Path::new("/repo/needle-shared.txt")
                    && file.input.read(cx).text().to_string() == "current adapter\n"
            })
        })
    });
    assert_eq!(fixture.new.read_count(ReadKind::Index), 1);
}

#[gpui_kit::test]
fn host_instance_directory_late_read_cannot_release_reconnected_request(cx: &mut TestAppContext) {
    let _serial = serial();
    let fixture = Replacement::new("old.txt", "new.txt", HostId::LOCAL);
    let old_instance = InstanceTicket::of(&fixture.old);
    let mut old_gate = fixture.old.pause(ReadKind::Directory);
    let (app, mut vcx, _stream) = fixture.window(cx);
    until(&mut vcx, "old directory read", |_| old_gate.entered());
    let mut new_gate = fixture.new.pause(ReadKind::Directory);
    activate(&app, &mut vcx, fixture.new.clone());
    // The replacement must start despite the identical HostId and path.
    until(&mut vcx, "replacement directory read", |_| {
        new_gate.entered()
    });
    let before = checkpoint(&app, &vcx, ReadKind::Directory);
    old_gate.release();
    until(&mut vcx, "retired directory callback", |vcx| {
        landed_since(&app, vcx, ReadKind::Directory, old_instance, before)
    });
    app.read_with(&vcx, |app, _| {
        assert!(
            app.file_tree
                .loads
                .is_pending(&(HostId::LOCAL, PathBuf::from("/repo"))),
            "old callback released the new same-key request (ABA)"
        );
        assert!(
            !app.file_tree
                .children
                .get(HostId::LOCAL, Path::new("/repo"))
                .is_some_and(|entries| entries.iter().any(|entry| entry.name == "old.txt"))
        );
    });
    new_gate.release();
    until(&mut vcx, "replacement directory result", |vcx| {
        listed(&app, vcx, HostId::LOCAL, "new.txt")
    });
    assert_rows(&mut vcx, "new.txt", "old.txt");
}

#[gpui_kit::test]
fn host_instance_directory_late_read_stays_out_of_another_host(cx: &mut TestAppContext) {
    let _serial = serial();
    let new_id = other_target().host_id();
    let fixture = Replacement::new("old.txt", "new.txt", new_id);
    let old_instance = InstanceTicket::of(&fixture.old);
    let mut gate = fixture.old.pause(ReadKind::Directory);
    let (app, mut vcx, _stream) = fixture.window(cx);
    until(&mut vcx, "old directory read", |_| gate.entered());
    activate(&app, &mut vcx, fixture.new.clone());
    until(&mut vcx, "new Host listing", |vcx| {
        listed(&app, vcx, new_id, "new.txt")
    });
    let before = checkpoint(&app, &vcx, ReadKind::Directory);
    gate.release();
    until(&mut vcx, "old Host callback", |vcx| {
        landed_since(&app, vcx, ReadKind::Directory, old_instance, before)
    });
    assert_rows(&mut vcx, "new.txt", "old.txt");
}

fn late_searches(cx: &mut TestAppContext, replacement_id: HostId) {
    let _serial = serial();
    let fixture = Replacement::new("needle-old.txt", "needle-new.txt", replacement_id);
    let old_instance = InstanceTicket::of(&fixture.old);
    let mut names = fixture.old.pause(ReadKind::Names);
    let mut contents = fixture.old.pause(ReadKind::Contents);
    let (app, mut vcx, _stream) = fixture.window(cx);
    until(&mut vcx, "initial listing", |vcx| {
        listed(&app, vcx, HostId::LOCAL, "needle-old.txt")
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.file_search
            .update(cx, |input, cx| input.focus(window, cx));
    });
    vcx.update(|window, cx| window.input("needle", cx));
    until(&mut vcx, "both old search reads", |_| {
        names.entered() && contents.entered()
    });
    activate(&app, &mut vcx, fixture.new.clone());
    until(&mut vcx, "replacement name and content searches", |vcx| {
        app.read_with(vcx, |app, _| {
            app.file_tree
                .search
                .hits
                .iter()
                .any(|hit| hit.name == "needle-new.txt")
                && !app.panel_search.run.running()
                && app.panel_search.run.landed().is_some_and(|landed| {
                    landed
                        .groups
                        .iter()
                        .any(|group| group.name == "needle-new.txt")
                })
        })
    });
    let before_names = checkpoint(&app, &vcx, ReadKind::Names);
    let before_contents = checkpoint(&app, &vcx, ReadKind::Contents);
    names.release();
    contents.release();
    until(&mut vcx, "both retired search callbacks", |vcx| {
        landed_since(&app, vcx, ReadKind::Names, old_instance, before_names)
            && landed_since(&app, vcx, ReadKind::Contents, old_instance, before_contents)
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.file_tree
                .search
                .hits
                .iter()
                .map(|hit| hit.name.as_str())
                .collect::<Vec<_>>(),
            ["needle-new.txt"]
        );
        let landed = app.panel_search.run.landed().unwrap();
        assert_eq!(landed.key.host, replacement_id);
        assert_eq!(
            landed
                .groups
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            ["needle-new.txt"]
        );
    });
    assert_rows(&mut vcx, "needle-new.txt", "needle-old.txt");
    vcx.update(|window, _| {
        assert!(
            window
                .find(SharedString::from("panel-search-file-/repo/needle-new.txt"))
                .visible()
        );
        assert!(
            window
                .try_find(SharedString::from("panel-search-file-/repo/needle-old.txt"))
                .is_none()
        );
    });
}

#[gpui_kit::test]
fn host_instance_same_query_searches_reread_reconnected_adapter(cx: &mut TestAppContext) {
    late_searches(cx, HostId::LOCAL);
}

#[gpui_kit::test]
fn host_instance_searches_keep_different_hosts_isolated(cx: &mut TestAppContext) {
    late_searches(cx, other_target().host_id());
}

fn index_names(app: &Entity<LyrelApp>, vcx: &VisualTestContext) -> Vec<String> {
    app.read_with(vcx, |app, cx| match app.file_list_now(cx) {
        FileList::Ready(index) => index
            .files
            .iter()
            .map(|file| file.name().to_owned())
            .collect(),
        _ => Vec::new(),
    })
}

#[gpui_kit::test]
fn host_instance_quick_open_reconnect_rewalks_unchanged_roots(cx: &mut TestAppContext) {
    let _serial = serial();
    let fixture = Replacement::new("old.txt", "new.txt", HostId::LOCAL);
    let old_instance = InstanceTicket::of(&fixture.old);
    let mut gate = fixture.old.pause(ReadKind::Index);
    let (app, mut vcx, _stream) = fixture.window(cx);
    until(&mut vcx, "initial directory", |vcx| {
        listed(&app, vcx, HostId::LOCAL, "old.txt")
    });
    app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
    until(&mut vcx, "old quick-open walk", |_| gate.entered());
    activate(&app, &mut vcx, fixture.new.clone());
    // The production successful-reconnect notification must refresh the
    // already-open consumer even though neither query nor cwd changed.
    app.update_in(&mut vcx, |app, window, cx| {
        app.file_tree_host_rebound(window, cx)
    });
    until(&mut vcx, "new quick-open walk", |vcx| {
        index_names(&app, vcx) == ["new.txt"]
    });
    let before = checkpoint(&app, &vcx, ReadKind::Index);
    gate.release();
    until(&mut vcx, "retired quick-open callback", |vcx| {
        landed_since(&app, vcx, ReadKind::Index, old_instance, before)
    });
    assert_eq!(index_names(&app, &vcx), ["new.txt"]);
    vcx.update(|window, cx| window.press("enter", cx));
    until(
        &mut vcx,
        "quick-open selected current adapter file",
        |vcx| {
            app.read_with(vcx, |app, _| {
                app.editor_active_location().is_some_and(|(host, path)| {
                    host == HostId::LOCAL && path == Path::new("/repo").join("new.txt")
                })
            })
        },
    );
    app.update_in(&mut vcx, |app, window, cx| {
        if app.search.view().is_none() {
            app.quick_open_file(window, cx);
        }
    });
    // Also retire a freshly landed walk, not just an in-flight one. Its age
    // must not suppress reading another adapter with the same cwd key.
    activate(
        &app,
        &mut vcx,
        NamespaceHost::new(HostId::LOCAL, fixture.old_dir.path()),
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.file_tree_host_rebound(window, cx)
    });
    until(&mut vcx, "replacement of fresh cached index", |vcx| {
        index_names(&app, vcx) == ["old.txt"]
    });
}

#[gpui_kit::test]
fn host_instance_repo_root_late_read_cannot_release_reconnected_resolution(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let fixture = Replacement::new("old.txt", "new.txt", HostId::LOCAL);
    // Only the retired namespace has a repository at its namespace root.
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(fixture.old_dir.path())
            .status()
            .unwrap()
            .success()
    );
    let old_instance = InstanceTicket::of(&fixture.old);
    let mut old_gate = fixture.old.pause(ReadKind::Root);
    let (app, mut vcx, _stream) = fixture.window(cx);
    until(&mut vcx, "old root read", |_| old_gate.entered());
    let mut new_gate = fixture.new.pause(ReadKind::Root);
    activate(&app, &mut vcx, fixture.new.clone());
    until(&mut vcx, "new root read", |_| new_gate.entered());
    let before = checkpoint(&app, &vcx, ReadKind::Root);
    old_gate.release();
    until(&mut vcx, "retired root callback", |vcx| {
        landed_since(&app, vcx, ReadKind::Root, old_instance, before)
    });
    app.read_with(&vcx, |app, _| {
        assert!(
            app.file_tree
                .repo_root_loads
                .is_pending(&(HostId::LOCAL, PathBuf::from("/repo")))
        );
        assert!(
            app.file_tree
                .repo_roots
                .get(HostId::LOCAL, Path::new("/repo"))
                .is_none()
        );
        assert!(app.file_tree.root_aliases.is_empty());
    });
    new_gate.release();
    until(&mut vcx, "new resolved root", |vcx| {
        app.update_in(vcx, |app, _, cx| {
            app.project_roots(cx).map(|(_, roots)| roots) == Some(vec![PathBuf::from("/repo")])
        })
    });
    until(&mut vcx, "new root listing", |vcx| {
        listed(&app, vcx, HostId::LOCAL, "new.txt")
    });
    assert_rows(&mut vcx, "new.txt", "old.txt");
}

#[gpui_kit::test]
fn host_instance_reconnect_watch_keeps_real_external_changes_fresh(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = temp();
    std::fs::write(root.path().join("before.txt"), "before").unwrap();
    let (app, mut vcx, _stream) = files_panel_on(cx, root.path());
    // An identity namespace delegates absolute desktop paths unchanged. This
    // tests the real watcher, without synthetic events or manual invalidation.
    let replacement: SharedHost = NamespaceHost::new(HostId::LOCAL, Path::new("/"));
    app.update_in(&mut vcx, |_, _, cx| {
        HostRegistry::insert(cx, replacement.clone());
    });
    until(&mut vcx, "replacement watcher", |vcx| {
        app.read_with(vcx, |app, _| {
            app.file_tree.watch.is_some()
                && app
                    .file_tree
                    .watch_host
                    .as_ref()
                    .is_some_and(|host| Arc::ptr_eq(host, &replacement))
        })
    });
    let created = root.path().join("external.txt");
    std::fs::write(&created, "external process writes after reconnect").unwrap();
    until(&mut vcx, "external change on replacement watcher", |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find(id(&created))
                .is_some_and(|row| row.visible())
        })
    });
}
