mod host;

use super::render_idle_gpui_tests::{files_panel_on, serial, settle};
use super::*;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{ClipboardEntry, ClipboardItem, Modifiers, TestAppContext, VisualTestContext};

fn primary() -> Modifiers {
    Modifiers {
        control: !cfg!(target_os = "macos"),
        platform: cfg!(target_os = "macos"),
        ..Default::default()
    }
}

fn id(path: &Path) -> SharedString {
    format!("tree-{}", path.display()).into()
}

fn click(vcx: &mut VisualTestContext, path: &Path, modifiers: Modifiers) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click_with_modifiers(id(path), modifiers, cx);
    });
}

fn key(vcx: &mut VisualTestContext, key: &str) {
    vcx.update(|window, cx| window.press(key, cx));
}

fn menu_item(vcx: &mut VisualTestContext, label: L10nKey) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let position = window.find_by_label(t(label)).bounds().center();
        window.pointer_down(position, MouseButton::Left, cx);
        window.pointer_up(position, MouseButton::Left, cx);
    });
}

fn selected(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Vec<PathBuf> {
    app.update_in(vcx, |app, _, cx| {
        let code = app.tab_code().unwrap();
        code.selected.targets(app.spawn_host(cx), &code.roots, true)
    })
}

fn wait(vcx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        vcx.background_executor.run_until_parked();
        if done(vcx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the file operation never completed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn temp() -> tempfile::TempDir {
    // Match the spelling reported by the daemon fixture on macOS and Windows.
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[gpui_kit::test]
fn ctrl_click_toggles_real_tree_rows_without_opening_and_shift_extends(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = temp();
    let a = root.path().join("a.txt");
    let b = root.path().join("b.txt");
    let c = root.path().join("c.txt");
    for path in [&a, &b, &c] {
        std::fs::write(path, "content").unwrap();
    }
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, &a, primary());
    click(&mut vcx, &b, primary());
    assert_eq!(selected(&app, &mut vcx), vec![a.clone(), b.clone()]);
    app.read_with(&vcx, |app, _| {
        assert!(
            app.tab_code().unwrap().active_id().is_none(),
            "modified clicks must not open a document"
        )
    });
    click(&mut vcx, &a, primary());
    assert_eq!(selected(&app, &mut vcx), vec![b.clone()]);
    click(&mut vcx, &c, Modifiers::shift());
    assert_eq!(
        selected(&app, &mut vcx),
        vec![a.clone(), b.clone(), c.clone()],
        "toggle establishes a new range anchor"
    );
    key(&mut vcx, "shift-up");
    assert_eq!(selected(&app, &mut vcx), vec![a.clone(), b.clone()]);
    key(&mut vcx, "down");
    assert_eq!(selected(&app, &mut vcx), vec![c.clone()]);
    click(&mut vcx, &b, Modifiers::none());
    wait(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.tab_code().unwrap().active_id().is_some())
    });
    assert_eq!(selected(&app, &mut vcx), vec![b]);
}

#[gpui_kit::test]
fn keyboard_range_selection_reveals_offscreen_files_and_keeps_them_operable(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = temp();
    for n in 0..160 {
        std::fs::write(root.path().join(format!("file-{n:03}.txt")), "content").unwrap();
    }
    let first = root.path().join("file-000.txt");
    let last = root.path().join("file-159.txt");
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, &first, primary());
    for _ in 0..159 {
        key(&mut vcx, "shift-down");
    }
    assert_eq!(selected(&app, &mut vcx).len(), 160);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let row = window.find(id(&last)).bounds();
        assert!(row.size.height > px(0.));
        assert!(row.origin.y >= px(0.));
        assert!(row.bottom() <= window.viewport_size().height);
    });
    key(&mut vcx, "up");
    assert_eq!(selected(&app, &mut vcx), vec![root.path().join("file-158.txt")]);
    key(&mut vcx, "enter");
    wait(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.tab_code().unwrap().active_id().is_some())
    });
}

#[gpui_kit::test]
fn inline_creation_and_rename_at_the_viewport_edge_keep_the_intended_directory(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = temp();
    for n in 0..80 {
        std::fs::create_dir(root.path().join(format!("dir-{n:03}"))).unwrap();
    }
    let destination = root.path().join("dir-079");
    let child = destination.join("created.txt");
    let renamed = destination.join("renamed.txt");
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, root.path(), primary());
    for _ in 0..80 {
        key(&mut vcx, "down");
    }
    assert_eq!(selected(&app, &mut vcx), vec![destination.clone()]);
    vcx.update(|window, cx| window.right_click(id(&destination), cx));
    menu_item(&mut vcx, L10nKey::FileTreeContextNewFile);
    vcx.update(|window, cx| window.input("created.txt", cx));
    key(&mut vcx, "enter");
    wait(&mut vcx, |_| child.exists());
    settle(&app, &mut vcx, root.path());
    assert!(!root.path().join("created.txt").exists());
    assert!(!root.path().join("dir-078/created.txt").exists());
    click(&mut vcx, &child, primary());
    vcx.update(|window, cx| window.right_click(id(&child), cx));
    menu_item(&mut vcx, L10nKey::FileTreeContextRename);
    key(&mut vcx, "secondary-a");
    vcx.update(|window, cx| window.input("renamed.txt", cx));
    key(&mut vcx, "enter");
    wait(&mut vcx, |_| renamed.exists() && !child.exists());
    assert_eq!(std::fs::read(&renamed).unwrap(), Vec::<u8>::new());
}

#[gpui_kit::test]
fn external_drop_targets_the_scrolled_empty_directory_not_its_neighbor(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = temp();
    let source = temp();
    let payload = source.path().join("payload.txt");
    std::fs::write(&payload, "drop bytes").unwrap();
    for n in 0..80 {
        std::fs::create_dir(root.path().join(format!("dir-{n:03}"))).unwrap();
    }
    let destination = root.path().join("dir-079");
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, root.path(), primary());
    for _ in 0..80 {
        key(&mut vcx, "down");
    }
    key(&mut vcx, "right");
    settle(&app, &mut vcx, root.path());
    let position = vcx.update(|window, cx| {
        window.scroll("right-panel-tree-rows", gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-200.))), cx);
        window.render_frame(cx);
        let row = window.find(id(&destination)).bounds();
        // The note immediately below the last directory is its empty child
        // area, and must not fall through to the root background target.
        gpui_kit::point(row.center().x, row.bottom() + px(TREE_ROW_H / 2.))
    });
    vcx.simulate_event(gpui_kit::FileDropEvent::Entered {
        position,
        paths: ExternalPaths(vec![payload.clone()].into()),
    });
    vcx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    wait(&mut vcx, |_| destination.join("payload.txt").exists());
    assert_eq!(std::fs::read_to_string(destination.join("payload.txt")).unwrap(), "drop bytes");
    assert!(payload.exists());
    assert!(!root.path().join("payload.txt").exists());
    assert!(!root.path().join("dir-078/payload.txt").exists());
    settle(&app, &mut vcx, root.path());
    let background = source.path().join("background.txt");
    std::fs::write(&background, "root bytes").unwrap();
    let position = vcx.update(|window, cx| {
        window.render_frame(cx);
        window.scroll("right-panel-tree-rows", gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-100_000.))), cx);
        window.render_frame(cx);
        let bounds = window.find("right-panel-tree-rows").bounds();
        gpui_kit::point(bounds.center().x, bounds.bottom() - px(4.))
    });
    vcx.simulate_event(gpui_kit::FileDropEvent::Entered {
        position,
        paths: ExternalPaths(vec![background].into()),
    });
    vcx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    wait(&mut vcx, |_| root.path().join("background.txt").exists());
    assert_eq!(std::fs::read_to_string(root.path().join("background.txt")).unwrap(), "root bytes");
    assert!(!destination.join("background.txt").exists());
}

#[gpui_kit::test]
fn late_ancestor_listing_reveals_the_file_and_external_insertions_keep_the_scroll_anchor(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = temp();
    let nested = root.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).unwrap();
    for n in 0..120 {
        std::fs::write(nested.join(format!("file-{n:03}.txt")), "bytes").unwrap();
    }
    let target = nested.join("file-090.txt");
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    app.update_in(&mut vcx, |app, window, cx| app.file_tree_show(&target, window, cx));
    wait(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id(&target)).is_some_and(|row| row.visible())
        })
    });
    settle(&app, &mut vcx, root.path());
    assert_eq!(selected(&app, &mut vcx), vec![target.clone()]);
    let before = vcx.update(|window, _| window.find(id(&target)).bounds());
    let inserted = nested.join("before-all.txt");
    std::fs::write(&inserted, "new bytes").unwrap();
    app.update_in(&mut vcx, |app, _, cx| {
        app.file_tree_apply_fs_events(HostId::LOCAL, &HashSet::from([inserted.clone()]), cx);
    });
    settle(&app, &mut vcx, root.path());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(id(&target)).bounds().origin.y, before.origin.y);
        // A deliberate scroll must not be pulled back by a completed reveal.
        window.scroll("right-panel-tree-rows", gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(100_000.))), cx);
        window.render_frame(cx);
        assert!(window.try_find(id(&target)).is_none());
    });
    assert_eq!(selected(&app, &mut vcx), vec![target]);
}

#[gpui_kit::test]
fn keyboard_copy_paste_copies_all_bytes_and_replacement_requires_confirmation(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = temp();
    let a = root.path().join("a.bin");
    let b = root.path().join("b.txt");
    let out = root.path().join("out");
    std::fs::write(&a, [0, 255, 3, 0, 128]).unwrap();
    std::fs::write(&b, "中文 copied\n").unwrap();
    std::fs::create_dir(&out).unwrap();
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, &a, primary());
    click(&mut vcx, &b, primary());
    key(&mut vcx, "secondary-c");
    click(&mut vcx, &out, Modifiers::none());
    key(&mut vcx, "secondary-v");
    wait(&mut vcx, |_| {
        std::fs::read(out.join("a.bin")).is_ok_and(|bytes| bytes == [0, 255, 3, 0, 128])
            && std::fs::read_to_string(out.join("b.txt")).is_ok_and(|text| text == "中文 copied\n")
    });
    settle(&app, &mut vcx, root.path());
    assert_eq!(
        std::fs::read(out.join("a.bin")).unwrap(),
        [0, 255, 3, 0, 128]
    );
    assert_eq!(
        std::fs::read_to_string(out.join("b.txt")).unwrap(),
        "中文 copied\n"
    );
    assert!(a.exists() && b.exists(), "copy must not move its sources");

    std::fs::write(out.join("b.txt"), "keep me").unwrap();
    key(&mut vcx, "secondary-v");
    wait(&mut vcx, |vcx| vcx.has_pending_prompt());
    assert_eq!(
        std::fs::read_to_string(out.join("b.txt")).unwrap(),
        "keep me"
    );
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    vcx.background_executor.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(out.join("b.txt")).unwrap(),
        "keep me"
    );
    key(&mut vcx, "secondary-v");
    wait(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::FileDropReplace));
    wait(&mut vcx, |_| {
        std::fs::read_to_string(out.join("b.txt")).is_ok_and(|text| text == "中文 copied\n")
    });

    // Replacing the OS clipboard retires the retained file payload. A plain
    // string that happens to spell a real path is not a copied file.
    vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(a.display().to_string())));
    std::fs::remove_file(out.join("a.bin")).unwrap();
    key(&mut vcx, "secondary-v");
    vcx.background_executor.run_until_parked();
    assert!(!out.join("a.bin").exists());
}

#[gpui_kit::test]
fn context_copy_and_delete_preserve_the_set_and_deduplicate_descendants(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = temp();
    let folder = root.path().join("bundle");
    let child = folder.join("child.txt");
    let sibling = root.path().join("sibling.txt");
    let out = root.path().join("out");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&out).unwrap();
    std::fs::write(&child, "nested bytes").unwrap();
    std::fs::write(&sibling, "sibling bytes").unwrap();
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, &folder, Modifiers::none());
    wait(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.file_tree.children.get(HostId::LOCAL, &folder).is_some()
        })
    });
    click(&mut vcx, &child, primary());
    click(&mut vcx, &sibling, primary());
    assert_eq!(
        selected(&app, &mut vcx),
        vec![folder.clone(), sibling.clone()]
    );
    vcx.update(|window, cx| window.right_click(id(&child), cx));
    assert_eq!(
        selected(&app, &mut vcx),
        vec![folder.clone(), sibling.clone()]
    );
    menu_item(&mut vcx, L10nKey::AppMenuCopy);
    // Paste from the actual row context menu, not a second command path.
    vcx.update(|window, cx| window.right_click(id(&out), cx));
    assert_eq!(
        selected(&app, &mut vcx),
        vec![out.clone()],
        "an unselected context target replaces the set"
    );
    menu_item(&mut vcx, L10nKey::AppMenuPaste);
    wait(&mut vcx, |_| {
        std::fs::read_to_string(out.join("bundle/child.txt"))
            .is_ok_and(|text| text == "nested bytes")
            && std::fs::read_to_string(out.join("sibling.txt"))
                .is_ok_and(|text| text == "sibling bytes")
    });
    assert_eq!(
        std::fs::read_to_string(out.join("bundle/child.txt")).unwrap(),
        "nested bytes"
    );

    click(&mut vcx, &out, primary()); // deselect the paste destination
    click(&mut vcx, &folder, primary());
    click(&mut vcx, &child, primary());
    click(&mut vcx, &sibling, primary());
    key(
        &mut vcx,
        if cfg!(target_os = "macos") {
            "cmd-backspace"
        } else {
            "delete"
        },
    );
    wait(&mut vcx, |vcx| vcx.has_pending_prompt());
    let (title, detail) = vcx.pending_prompt().unwrap();
    assert_eq!(
        title,
        t_fmt(L10nKey::FileTreeDeleteManyTitle, &[("n", "2")])
    );
    assert!(detail.contains(&folder.display().to_string()));
    assert!(detail.contains(&sibling.display().to_string()));
    assert!(
        !detail.contains(&child.display().to_string()),
        "the child is already covered by its folder"
    );
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    vcx.background_executor.run_until_parked();
    assert!(child.exists() && sibling.exists());
    // Menu's final destructive item shares the keyboard owner's snapshot.
    vcx.update(|window, cx| window.right_click(id(&child), cx));
    key(&mut vcx, "up");
    key(&mut vcx, "enter");
    wait(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::Delete));
    wait(&mut vcx, |_| !folder.exists() && !sibling.exists());
    assert_eq!(
        std::fs::read_to_string(out.join("bundle/child.txt")).unwrap(),
        "nested bytes"
    );
}

#[test]
fn tree_selection_is_tab_and_host_scoped_and_protects_roots() {
    let mut first = crate::ui::code_editor::TabCode::new();
    let second = crate::ui::code_editor::TabCode::new();
    let local = HostId::LOCAL;
    let remote = HostId(512);
    let path = PathBuf::from("/workspace/item");
    first.selected.single(local, path.clone());
    assert!(!second.selected.contains(local, &path));
    assert!(!first.selected.contains(remote, &path));
    assert!(first.selected.targets(remote, &[], true).is_empty());
    first.selected.single(remote, path.clone());
    assert!(!first.selected.contains(local, &path));
    assert_eq!(
        first.selected.targets(remote, &[], true),
        vec![path.clone()]
    );
    assert!(
        first
            .selected
            .targets(remote, &[path.clone()], false)
            .is_empty()
    );
    first.selected.clear();
    assert!(first.selected.cursor(remote).is_none());
}

#[test]
fn copies_use_source_and_destination_hosts_even_for_identical_wire_names() {
    use lyrel_core::host::Host as _;
    let source_root = temp();
    let destination_root = temp();
    std::fs::create_dir_all(source_root.path().join("project/pkg/deep")).unwrap();
    std::fs::write(
        source_root.path().join("project/pkg/deep/a.bin"),
        [0, 255, 128],
    )
    .unwrap();
    std::fs::create_dir_all(destination_root.path().join("project/pkg/into")).unwrap();
    let source = host::NamespaceHost::new(HostId(601), source_root.path());
    let destination = host::NamespaceHost::new(HostId(602), destination_root.path());
    let sources = vec![
        PathBuf::from("/project/pkg"),
        PathBuf::from("/project/missing.txt"),
    ];
    let target = Path::new("/project/pkg/into");
    let report = file_copy::copy_between_hosts(
        source.as_ref(),
        destination.as_ref(),
        &sources,
        target,
        false,
    );
    assert_eq!(report.copied, ["pkg"]);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].0, "missing.txt");
    assert_eq!(
        std::fs::read(
            destination_root
                .path()
                .join("project/pkg/into/pkg/deep/a.bin")
        )
        .unwrap(),
        [0, 255, 128]
    );
    assert!(!source_root.path().join("project/pkg/into").exists());

    // Same spelling on the same Host is genuinely recursive and must fail.
    let report = file_copy::copy_between_hosts(
        source.as_ref(),
        source.as_ref(),
        &sources[..1],
        Path::new("/project/pkg/deep"),
        false,
    );
    assert!(report.copied.is_empty());
    assert_eq!(report.errors.len(), 1);

    // Replacing a remote directory is the same staged transaction as a drop;
    // it replaces rather than merges and leaves unrelated source bytes alone.
    destination
        .write_file(Path::new("/project/pkg/into/pkg/stale.txt"), b"stale")
        .unwrap();
    let report = file_copy::copy_between_hosts(
        source.as_ref(),
        destination.as_ref(),
        &sources[..1],
        target,
        false,
    );
    assert_eq!(report.conflicts, ["pkg"]);
    let report = file_copy::copy_between_hosts(
        source.as_ref(),
        destination.as_ref(),
        &sources[..1],
        target,
        true,
    );
    assert_eq!(report.copied, ["pkg"]);
    assert!(report.errors.is_empty());
    assert!(
        !destination_root
            .path()
            .join("project/pkg/into/pkg/stale.txt")
            .exists()
    );
    assert_eq!(
        std::fs::read(source_root.path().join("project/pkg/deep/a.bin")).unwrap(),
        [0, 255, 128]
    );
}

#[gpui_kit::test]
fn a_partial_paste_keeps_successful_files_and_does_not_silently_retry_other_hosts(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = temp();
    let sources = temp();
    let good = sources.path().join("good.txt");
    let missing = sources.path().join("missing.txt");
    std::fs::write(&good, "survives another source failing").unwrap();
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, root.path(), primary());
    vcx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                vec![good, missing].into(),
            ))],
        })
    });
    key(&mut vcx, "secondary-v");
    wait(&mut vcx, |vcx| {
        root.path().join("good.txt").exists()
            && vcx.read(|cx| {
                crate::ui::notification_history::history(cx)
                    .read(cx)
                    .records()
                    .any(|record| record.message().contains("missing.txt"))
            })
    });
    settle(&app, &mut vcx, root.path());
    assert_eq!(
        std::fs::read_to_string(root.path().join("good.txt")).unwrap(),
        "survives another source failing"
    );
    assert!(!root.path().join("missing.txt").exists());
    app.read_with(&vcx, |app, _| {
        assert!(
            app.file_tree
                .children
                .get(HostId::LOCAL, root.path())
                .unwrap()
                .iter()
                .any(|entry| entry.name == "good.txt")
        );
    });
}

#[gpui_kit::test]
fn a_failed_batch_delete_names_its_target_and_keeps_successful_removals(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = temp();
    let first = root.path().join("first.txt");
    let vanished = root.path().join("vanished.txt");
    std::fs::write(&first, "remove me").unwrap();
    std::fs::write(&vanished, "external deletion").unwrap();
    let (app, mut vcx, _pane) = files_panel_on(cx, root.path());
    click(&mut vcx, &first, primary());
    click(&mut vcx, &vanished, primary());
    key(
        &mut vcx,
        if cfg!(target_os = "macos") {
            "cmd-backspace"
        } else {
            "delete"
        },
    );
    wait(&mut vcx, |vcx| vcx.has_pending_prompt());
    // Another tool removes one target while the user is deciding. The batch
    // must still finish its other target and report the exact failing name.
    std::fs::remove_file(&vanished).unwrap();
    vcx.simulate_prompt_answer(t(L10nKey::Delete));
    wait(&mut vcx, |vcx| {
        !first.exists()
            && vcx.read(|cx| {
                crate::ui::notification_history::history(cx)
                    .read(cx)
                    .records()
                    .any(|record| record.message().contains("vanished.txt"))
            })
    });
    assert!(root.path().exists());
    app.read_with(&vcx, |app, _| {
        let code = app.tab_code().unwrap();
        assert!(!code.selected.contains(HostId::LOCAL, &first));
    });
}
