use super::render_idle_gpui_tests::{files_panel_on, serial, settle};
use super::*;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::time::{Duration, Instant};

fn pump(vcx: &mut VisualTestContext, duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        vcx.background_executor.run_until_parked();
        vcx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(10));
    }
    vcx.background_executor.run_until_parked();
}

#[gpui_kit::test]
fn reopening_a_cached_folder_shows_files_created_while_collapsed(cx: &mut TestAppContext) {
    let _serial = serial();
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let docs = root.join("docs");
    std::fs::create_dir(&docs).unwrap();
    std::fs::write(docs.join("before.md"), "before\n").unwrap();
    let (app, mut vcx, _pane) = files_panel_on(cx, &root);
    app.update_in(&mut vcx, |app, _, cx| {
        app.file_tree_toggle_expand(&docs, cx);
    });
    settle(&app, &mut vcx, &root);
    pump(&mut vcx, Duration::from_millis(300));
    assert!(app.update_in(&mut vcx, |app, _, _| {
        app.file_tree
            .children
            .get(HostId::LOCAL, &docs)
            .is_some_and(|entries| entries.iter().any(|entry| entry.name == "before.md"))
    }));
    app.update_in(&mut vcx, |app, _, cx| {
        app.file_tree_toggle_expand(&docs, cx);
    });
    pump(&mut vcx, Duration::from_millis(300));
    let created = docs.join("agent-created.md");
    std::fs::write(&created, "created by an external process\n").unwrap();
    pump(&mut vcx, Duration::from_millis(500));
    app.update_in(&mut vcx, |app, _, cx| {
        app.file_tree_toggle_expand(&docs, cx);
    });
    settle(&app, &mut vcx, &root);
    let visible: Vec<PathBuf> = app.update_in(&mut vcx, |app, _, _| {
        let code = app.tab_code().unwrap();
        app.file_tree
            .visible_rows(HostId::LOCAL, &code.roots, &code.expanded)
            .into_iter()
            .filter(|row| row.note.is_none())
            .map(|row| row.entry.path)
            .collect()
    });
    assert!(
        visible.contains(&created),
        "new file must be visible after reopening a cached folder; visible: {visible:?}"
    );
}

#[gpui_kit::test]
fn a_debounce_batch_invalidates_every_host_without_dropping_events(cx: &mut TestAppContext) {
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let local = HostId::LOCAL;
    let remote = HostId(987);
    let dir = PathBuf::from("/project/docs");
    app.update_in(&mut vcx, |app, _, _| {
        for host in [local, remote] {
            app.file_tree.children.insert(host, dir.clone(), Vec::new());
            app.file_tree
                .events_tx
                .try_send((host, vec![dir.join("agent-created.md")]))
                .unwrap();
        }
    });
    pump(&mut vcx, Duration::from_millis(100));
    app.update_in(&mut vcx, |app, _, _| {
        assert!(app.file_tree.stale.contains(&(local, dir.clone())));
        assert!(
            app.file_tree.stale.contains(&(remote, dir.clone())),
            "another host's event in the same debounce must not be discarded"
        );
    });
}

#[gpui_kit::test]
fn a_late_watch_open_cannot_replace_the_current_host_connection(cx: &mut TestAppContext) {
    let previous = lyrel_core::host::local::LocalHost::new();
    let current = lyrel_core::host::local::LocalHost::new();
    let sub = std::thread::spawn(|| lyrel_core::host::local::LocalHost::new().watch(&[]))
        .join()
        .unwrap()
        .unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    app.update_in(&mut vcx, |app, _, _| {
        app.file_tree.watch_host = Some(current.clone());
        app.file_tree.watch_opening = true;
        let landed = app
            .file_tree
            .finish_watch_open(Ok(Arc::new(sub)), previous, &HashSet::new());
        assert!(
            landed.is_none(),
            "a retired connection's late watch must not be adopted"
        );
        assert!(Arc::ptr_eq(
            app.file_tree.watch_host.as_ref().unwrap(),
            &current
        ));
        assert!(
            app.file_tree.watch_opening,
            "the current connection is still opening its own watch"
        );
    });
}

#[gpui_kit::test]
fn a_directory_event_refreshes_that_directorys_cached_children(cx: &mut TestAppContext) {
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    app.update_in(&mut vcx, |app, _, _| {
        let dir = PathBuf::from("/project/docs");
        app.file_tree
            .children
            .insert(HostId::LOCAL, dir.clone(), Vec::new());
        let refresh = app.file_tree.apply_fs_events(
            HostId::LOCAL,
            &HashSet::from([dir.clone()]),
            true,
            true,
            HostId::LOCAL,
        );
        assert!(
            refresh.reload,
            "a directory-only notification must re-read its children"
        );
        assert!(app.file_tree.stale.contains(&(HostId::LOCAL, dir)));
    });
}
