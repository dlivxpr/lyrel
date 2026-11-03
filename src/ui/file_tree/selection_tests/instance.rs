//! Real Host reads land through HostOps into the production workbench. Gates
//! stop a completed backing read before its UI callback, not an executor queue.
use super::host::{NamespaceHost, ReadKind};
use super::*;
use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WorkspaceId, WorkspaceStore};
use crate::terminal::PaneWorkspace;
use crate::ui::app::test_window::harness_with_pane;
use crate::ui::search::files::FileList;
use std::time::{Duration, Instant};

fn backing(name: &str) -> tempfile::TempDir {
    let temp = temp();
    std::fs::create_dir(temp.path().join("repo")).unwrap();
    std::fs::write(temp.path().join("repo").join(name), "needle in real contents\n").unwrap();
    temp
}

fn other_target() -> RemoteTarget {
    RemoteTarget::Alias { alias: "instance-fence-other-host".into() }
}

fn activate(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, host: SharedHost) {
    app.update_in(vcx, |app, _, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap().terminal().unwrap().clone();
        if !host.id().is_local() {
            let target = other_target();
            assert_eq!(target.host_id(), host.id());
            let workspace = WorkspaceId::new();
            let view = WindowView::on_remote(RemoteRef::new(target.clone(), workspace));
            app.workspace = view.id;
            let mut views = WorkspaceStore::all(cx).clone();
            views.views.push(view);
            WorkspaceStore::install_for_test(cx, views);
            pane.update(cx, |pane, _| pane.set_workspace(Some(PaneWorkspace {
                workspace, target, spec: None, label: None, resize_echo: false,
            })));
        }
        HostRegistry::insert(cx, host);
        pane.update(cx, |pane, _| pane.set_git_status_cwd_for_test(Some(PathBuf::from("/repo"))));
        app.set_right_panel_tab(RightPanelTab::Files, cx);
        cx.notify();
    });
}

#[track_caller]
fn until(vcx: &mut VisualTestContext, stage: &str, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        vcx.executor().advance_clock(Duration::from_millis(300));
        vcx.background_executor.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        if ready(vcx) { return; }
        assert!(Instant::now() < deadline, "{stage} never completed");
        std::thread::yield_now();
    }
}

fn listed(app: &Entity<LyrelApp>, vcx: &VisualTestContext, id: HostId, name: &str) -> bool {
    app.read_with(vcx, |app, _| {
        app.file_tree.children.get(id, Path::new("/repo"))
            .is_some_and(|entries| entries.iter().any(|entry| entry.name == name))
            && !app.file_tree.loads.is_pending(&(id, PathBuf::from("/repo")))
    })
}

fn assert_rows(vcx: &mut VisualTestContext, present: &str, absent: &str) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find(id(Path::new(&format!("/repo/{present}")))).visible());
        assert!(window.try_find(id(Path::new(&format!("/repo/{absent}")))).is_none());
    });
}

#[gpui_kit::test]
fn host_instance_directory_late_read_cannot_release_reconnected_request(cx: &mut TestAppContext) {
    let _serial = serial();
    let old_dir = backing("old.txt");
    let new_dir = backing("new.txt");
    let old = NamespaceHost::new(HostId::LOCAL, old_dir.path());
    let old_instance = Arc::as_ptr(&old) as *const () as usize;
    let new = NamespaceHost::new(HostId::LOCAL, new_dir.path());
    let mut old_gate = old.pause(ReadKind::Directory);
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    activate(&app, &mut vcx, old);
    until(&mut vcx, "old directory read", |_| old_gate.entered());
    let mut new_gate = new.pause(ReadKind::Directory);
    activate(&app, &mut vcx, new);
    // The replacement must start despite the identical HostId and path.
    until(&mut vcx, "replacement directory read", |_| new_gate.entered());
    let before = app.read_with(&vcx, |app, _| app.file_tree.read_landings[0]);
    old_gate.release();
    until(&mut vcx, "retired directory callback", |vcx| {
        app.read_with(vcx, |app, _| app.file_tree.read_landings[0] > before
            && app.file_tree.read_landing_hosts[0] == Some(old_instance))
    });
    app.read_with(&vcx, |app, _| {
        assert!(app.file_tree.loads.is_pending(&(HostId::LOCAL, PathBuf::from("/repo"))),
            "old callback released the new same-key request (ABA)");
        assert!(!app.file_tree.children.get(HostId::LOCAL, Path::new("/repo"))
            .is_some_and(|entries| entries.iter().any(|entry| entry.name == "old.txt")));
    });
    new_gate.release();
    until(&mut vcx, "replacement directory result", |vcx| listed(&app, vcx, HostId::LOCAL, "new.txt"));
    assert_rows(&mut vcx, "new.txt", "old.txt");
}

#[gpui_kit::test]
fn host_instance_directory_late_read_stays_out_of_another_host(cx: &mut TestAppContext) {
    let _serial = serial();
    let old_dir = backing("old.txt");
    let new_dir = backing("new.txt");
    let old = NamespaceHost::new(HostId::LOCAL, old_dir.path());
    let old_instance = Arc::as_ptr(&old) as *const () as usize;
    let new_id = other_target().host_id();
    let new = NamespaceHost::new(new_id, new_dir.path());
    let mut gate = old.pause(ReadKind::Directory);
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    activate(&app, &mut vcx, old);
    until(&mut vcx, "old directory read", |_| gate.entered());
    activate(&app, &mut vcx, new);
    until(&mut vcx, "new Host listing", |vcx| listed(&app, vcx, new_id, "new.txt"));
    let before = app.read_with(&vcx, |app, _| app.file_tree.read_landings[0]);
    gate.release();
    until(&mut vcx, "old Host callback", |vcx| app.read_with(vcx, |app, _| {
        app.file_tree.read_landings[0] > before && app.file_tree.read_landing_hosts[0] == Some(old_instance)
    }));
    assert_rows(&mut vcx, "new.txt", "old.txt");
}

fn late_searches(cx: &mut TestAppContext, replacement_id: HostId) {
    let _serial = serial();
    let old_dir = backing("needle-old.txt");
    let new_dir = backing("needle-new.txt");
    let old = NamespaceHost::new(HostId::LOCAL, old_dir.path());
    let old_instance = Arc::as_ptr(&old) as *const () as usize;
    let new = NamespaceHost::new(replacement_id, new_dir.path());
    let mut names = old.pause(ReadKind::Names);
    let mut contents = old.pause(ReadKind::Contents);
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    activate(&app, &mut vcx, old);
    until(&mut vcx, "initial listing", |vcx| listed(&app, vcx, HostId::LOCAL, "needle-old.txt"));
    app.update_in(&mut vcx, |app, window, cx| {
        app.file_search.update(cx, |input, cx| input.focus(window, cx));
    });
    vcx.update(|window, cx| window.input("needle", cx));
    until(&mut vcx, "both old search reads", |_| names.entered() && contents.entered());
    activate(&app, &mut vcx, new);
    until(&mut vcx, "replacement name and content searches", |vcx| {
        app.read_with(vcx, |app, _| {
            app.file_tree.search.hits.iter().any(|hit| hit.name == "needle-new.txt")
                && !app.panel_search.run.running()
                && app.panel_search.run.landed().is_some_and(|landed| {
                    landed.groups.iter().any(|group| group.name == "needle-new.txt")
                })
        })
    });
    let before = app.read_with(&vcx, |app, _| (app.file_tree.read_landings[1], app.panel_search.read_landings));
    names.release();
    contents.release();
    until(&mut vcx, "both retired search callbacks", |vcx| app.read_with(vcx, |app, _| {
        app.file_tree.read_landings[1] > before.0 && app.panel_search.read_landings > before.1
            && app.file_tree.read_landing_hosts[1] == Some(old_instance)
            && app.panel_search.read_landing_host == Some(old_instance)
    }));
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.file_tree.search.hits.iter().map(|hit| hit.name.as_str()).collect::<Vec<_>>(), ["needle-new.txt"]);
        let landed = app.panel_search.run.landed().unwrap();
        assert_eq!(landed.key.host, replacement_id);
        assert_eq!(landed.groups.iter().map(|group| group.name.as_str()).collect::<Vec<_>>(), ["needle-new.txt"]);
    });
    assert_rows(&mut vcx, "needle-new.txt", "needle-old.txt");
    vcx.update(|window, _| {
        assert!(window.find(SharedString::from("panel-search-file-/repo/needle-new.txt")).visible());
        assert!(window.try_find(SharedString::from("panel-search-file-/repo/needle-old.txt")).is_none());
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
        FileList::Ready(index) => index.files.iter().map(|file| file.name().to_owned()).collect(),
        _ => Vec::new(),
    })
}

#[gpui_kit::test]
fn host_instance_quick_open_reconnect_rewalks_unchanged_roots(cx: &mut TestAppContext) {
    let _serial = serial();
    let old_dir = backing("old.txt");
    let new_dir = backing("new.txt");
    let old = NamespaceHost::new(HostId::LOCAL, old_dir.path());
    let old_instance = Arc::as_ptr(&old) as *const () as usize;
    let new = NamespaceHost::new(HostId::LOCAL, new_dir.path());
    let mut gate = old.pause(ReadKind::Index);
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    activate(&app, &mut vcx, old);
    until(&mut vcx, "initial directory", |vcx| listed(&app, vcx, HostId::LOCAL, "old.txt"));
    app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
    until(&mut vcx, "old quick-open walk", |_| gate.entered());
    activate(&app, &mut vcx, new);
    // Exercise the real quick-open refresh, with exactly the same cwd key.
    app.update_in(&mut vcx, |app, window, cx| app.refresh_file_index(window, cx));
    until(&mut vcx, "new quick-open walk", |vcx| index_names(&app, vcx) == ["new.txt"]);
    let before = app.read_with(&vcx, |app, _| app.file_tree.read_landings[3]);
    gate.release();
    until(&mut vcx, "retired quick-open callback", |vcx| app.read_with(vcx, |app, _| {
        app.file_tree.read_landings[3] > before && app.file_tree.read_landing_hosts[3] == Some(old_instance)
    }));
    assert_eq!(index_names(&app, &vcx), ["new.txt"]);
    vcx.update(|window, cx| window.press("enter", cx));
    until(&mut vcx, "quick-open selected current adapter file", |vcx| {
        app.read_with(vcx, |app, _| app.editor_active_location().is_some_and(|(host, path)| {
            host == HostId::LOCAL && path == Path::new("/repo").join("new.txt")
        }))
    });
    // Also retire a freshly landed walk, not just an in-flight one. Its age
    // must not suppress reading another adapter with the same cwd key.
    activate(&app, &mut vcx, NamespaceHost::new(HostId::LOCAL, old_dir.path()));
    app.update_in(&mut vcx, |app, window, cx| app.refresh_file_index(window, cx));
    until(&mut vcx, "replacement of fresh cached index", |vcx| index_names(&app, vcx) == ["old.txt"]);
}

#[gpui_kit::test]
fn host_instance_repo_root_late_read_cannot_release_reconnected_resolution(cx: &mut TestAppContext) {
    let _serial = serial();
    let old_dir = backing("old.txt");
    let new_dir = backing("new.txt");
    // Only the retired namespace has a repository at its namespace root.
    assert!(std::process::Command::new("git").args(["init", "--quiet"])
        .arg(old_dir.path()).status().unwrap().success());
    let old = NamespaceHost::new(HostId::LOCAL, old_dir.path());
    let old_instance = Arc::as_ptr(&old) as *const () as usize;
    let new = NamespaceHost::new(HostId::LOCAL, new_dir.path());
    let mut old_gate = old.pause(ReadKind::Root);
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    activate(&app, &mut vcx, old);
    until(&mut vcx, "old root read", |_| old_gate.entered());
    let mut new_gate = new.pause(ReadKind::Root);
    activate(&app, &mut vcx, new);
    until(&mut vcx, "new root read", |_| new_gate.entered());
    let before = app.read_with(&vcx, |app, _| app.file_tree.read_landings[2]);
    old_gate.release();
    until(&mut vcx, "retired root callback", |vcx| app.read_with(vcx, |app, _| {
        app.file_tree.read_landings[2] > before && app.file_tree.read_landing_hosts[2] == Some(old_instance)
    }));
    app.read_with(&vcx, |app, _| {
        assert!(app.file_tree.repo_root_loads.is_pending(&(HostId::LOCAL, PathBuf::from("/repo"))));
        assert!(app.file_tree.repo_roots.get(HostId::LOCAL, Path::new("/repo")).is_none());
        assert!(app.file_tree.root_aliases.is_empty());
    });
    new_gate.release();
    until(&mut vcx, "new resolved root", |vcx| app.update_in(vcx, |app, _, cx| {
        app.project_roots(cx).map(|(_, roots)| roots) == Some(vec![PathBuf::from("/repo")])
    }));
    until(&mut vcx, "new root listing", |vcx| listed(&app, vcx, HostId::LOCAL, "new.txt"));
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
    app.update_in(&mut vcx, |_, _, cx| { HostRegistry::insert(cx, replacement.clone()); });
    until(&mut vcx, "replacement watcher", |vcx| app.read_with(vcx, |app, _| {
        app.file_tree.watch.is_some() && app.file_tree.watch_host.as_ref()
            .is_some_and(|host| Arc::ptr_eq(host, &replacement))
    }));
    let created = root.path().join("external.txt");
    std::fs::write(&created, "external process writes after reconnect").unwrap();
    until(&mut vcx, "external change on replacement watcher", |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id(&created)).is_some_and(|row| row.visible())
        })
    });
}
