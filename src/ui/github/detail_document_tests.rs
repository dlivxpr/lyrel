// The issue/PR detail as a document: opened from the list into the document
// area, read and dismissed with the keyboard, docked and filled like any
// other document, and closed without touching the file it covered.

/// A repository with the shared `Fake` fixture transport, the GitHub panel
/// open, and the list settled — the common half of every detail test.
fn detail_doc_panel(
    cx: &mut TestAppContext,
    kind: Kind,
) -> (
    tempfile::TempDir,
    PathBuf,
    Entity<LyrelApp>,
    VisualTestContext,
    crate::daemon::transport::Stream,
) {
    let checkout = tempfile::tempdir().unwrap();
    let root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(checkout.path()).unwrap(),
    );
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &["remote", "add", "origin", "https://github.com/acme/widgets"],
    );
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    let transport: Arc<dyn Transport> = Arc::new(Fake {
        asked: Mutex::new(Vec::new()),
    });
    app.update_in(&mut vcx, |app, _, cx| {
        app.github.connector = Some(Arc::new(move || Connection {
            transport: transport.clone(),
        }));
        app.github.kind = kind;
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::GitHub;
        cx.notify();
    });
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    settle(&app, &mut vcx, "the requested GitHub list", |app| {
        app.github
            .lists
            .iter()
            .any(|(q, l)| q.slug.full() == "acme/widgets" && q.kind == kind && l.loaded)
    });
    (checkout, root, app, vcx, pane)
}

#[gpui_kit::test]
fn enter_opens_the_detail_document_and_escape_returns_to_the_list(cx: &mut TestAppContext) {
    let (_checkout, root, app, mut vcx, _pane) = detail_doc_panel(cx, Kind::Issues);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let list = window.find("panel-github-rows-acme/widgets").bounds();
        let first = window.find("panel-github-item-7").bounds();
        window.click_at(
            "panel-github-rows-acme/widgets",
            gpui_kit::point(
                (first.left() - list.left()) / 2.,
                first.center().y - list.top(),
            ),
            cx,
        );
        window.press("down", cx);
        assert_eq!(window.find("panel-github-item-7").selected(), Some(true));
        window.press("enter", cx);
    });
    // The explicit keyboard open lands in the document area, focus included,
    // while the list stays in the panel with its selection untouched.
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("github-detail-document").visible());
        assert_eq!(window.find("github-detail-document").focused(), Some(true));
        assert!(window.find("github-detail-header").visible());
        assert_eq!(window.find("panel-github-item-7").selected(), Some(true));
        // Read-only GitHub content carries no file chrome.
        assert!(window.try_find("code-status-bar").is_none());
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| app.github.open.as_ref().map(|(_, n)| *n)),
        Some(7)
    );

    settle(&app, &mut vcx, "the detail body", |app| {
        app.github.details.values().any(|d| d.detail.is_some())
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find("github-detail-scroll").visible(),
            "the fetched detail renders under the document header"
        );
        // Escape closes the document; with no file underneath, the list
        // regains the keyboard.
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("github-detail-document").is_none());
        assert_eq!(
            window.find("panel-github-rows-acme/widgets").focused(),
            Some(true),
            "closing with no prior document returns to the selected list item"
        );
        assert_eq!(window.find("panel-github-item-7").selected(), Some(true));
    });
    assert!(app.read_with(&vcx, |app, _| app.github.open.is_none()));
    test_window::quiesce(&mut vcx, Some(&root));
}

#[gpui_kit::test]
fn clicking_a_row_opens_the_detail_over_a_file_and_close_restores_it(cx: &mut TestAppContext) {
    let (_checkout, _root, app, mut vcx, _pane) = detail_doc_panel(cx, Kind::Issues);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello").unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let editor = loop {
        vcx.run_until_parked();
        if let Some(file) = app.read_with(&vcx, |app, cx| app.editor_active_local_file(cx)) {
            break file.into_input();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fixture file must load"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // An unsaved edit, the way an accepted suggestion lands.
    vcx.update(|window, cx| {
        window.press("end", cx);
        window.input("!", cx);
    });
    assert_eq!(
        editor.read_with(&vcx, |editor, _| editor.value().to_string()),
        "hello!"
    );
    let tab = app.read_with(&vcx, |app, _| {
        app.tabs.get(app.tabs.active_index()).map(|tab| tab.id())
    });

    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-7", cx);
        window.render_frame(cx);
        assert!(window.find("github-detail-document").visible());
        assert!(
            window.try_find("code-status-bar").is_none(),
            "the modified file rests untouched under the read-only surface"
        );
    });

    // Closing the detail restores the covered document: its edit, and input
    // focus — nothing was saved or discarded by the round trip.
    vcx.update(|window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("github-detail-document").is_none());
        assert!(window.find("code-status-bar").visible());
    });
    assert_eq!(
        app.read_with(&vcx, |app, cx| app.editor.read(cx).had_focus()),
        tab,
        "input focus returns to the editor the detail covered"
    );
    assert_eq!(
        editor.read_with(&vcx, |editor, _| editor.value().to_string()),
        "hello!"
    );
}

#[gpui_kit::test]
fn the_detail_document_docks_and_fills_like_other_documents(cx: &mut TestAppContext) {
    let (_checkout, root, _app, mut vcx, _pane) = detail_doc_panel(cx, Kind::Issues);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-7", cx);
        window.render_frame(cx);
        // Docked beside the terminal, the document takes the column's width.
        let docked = window.find("github-detail-document").bounds();
        window.click(("document-layout", 1usize), cx);
        window.render_frame(cx);
        let filled = window.find("github-detail-document").bounds();
        assert!(
            filled.size.width > docked.size.width,
            "fill covers the workspace; the dock takes the column beside the terminal"
        );
        window.click(("document-layout", 0usize), cx);
        window.render_frame(cx);
        let again = window.find("github-detail-document").bounds();
        assert_eq!(
            again.size.width, docked.size.width,
            "asking for the dock again restores the requested split"
        );
    });
    test_window::quiesce(&mut vcx, Some(&root));
}

/// The full review round trip from #355: an unsaved file stays exactly as it
/// was while its reader follows a pull request's changed file into the patch
/// and back out through the detail. The patch is the one GitHub sent for
/// that pull request — never a local working-copy diff — and each Escape
/// hands the keyboard to the surface underneath.
#[gpui_kit::test]
fn a_pr_patch_round_trips_through_its_detail_back_to_the_unsaved_document(cx: &mut TestAppContext) {
    let (_checkout, root, app, mut vcx, _pane) = detail_doc_panel(cx, Kind::Pulls);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello").unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let editor = loop {
        vcx.run_until_parked();
        if let Some(file) = app.read_with(&vcx, |app, cx| app.editor_active_local_file(cx)) {
            break file.into_input();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fixture file must load"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // An unsaved edit, the way an accepted suggestion lands.
    vcx.update(|window, cx| {
        window.press("end", cx);
        window.input("!", cx);
    });
    assert_eq!(
        editor.read_with(&vcx, |editor, _| editor.value().to_string()),
        "hello!"
    );
    let tab = app.read_with(&vcx, |app, _| {
        app.tabs.get(app.tabs.active_index()).map(|tab| tab.id())
    });

    let open_pr = |app: &gpui_kit::Entity<LyrelApp>, vcx: &mut VisualTestContext| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.click("panel-github-item-8", cx);
        });
        let key = app.read_with(vcx, |app, _| {
            app.github
                .lists
                .keys()
                .next()
                .map(|q| (q.slug.clone(), 8))
                .expect("the settled list names its repository")
        });
        settle(app, vcx, "the pull request's detail", |app| {
            app.github
                .details
                .get(&key)
                .and_then(|d| d.detail.as_ref())
                .and_then(|d| d.files.as_ref())
                .is_some_and(|files| !files.is_empty())
        });
        key
    };

    let key = open_pr(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        // What the pull request is read for stays reachable in the document.
        assert!(window.find("github-detail-refresh").visible());
        assert!(window.find("panel-github-open-item").visible());
        window.click(("panel-github-file", 0usize), cx);
        window.render_frame(cx);
    });

    // The patch on screen is the one the pull request supplied: its source
    // names the originating repository and item, its directory is the
    // checkout that item was opened from, and its rows carry the fixture's
    // marker line — content the empty local fixture could never produce.
    app.read_with(&vcx, |app, _| {
        let overlay = app.tabs[app.tabs.active_index()]
            .diff_overlay
            .as_ref()
            .expect("the file row opened the patch");
        assert!(
            overlay.source.is_supplied(),
            "a PR file opens its supplied patch, not a local working-copy diff"
        );
        match &overlay.source {
            crate::ui::scm::diff::DiffSource::Patch { id, .. } => {
                assert_eq!(id, "acme/widgets#8");
            }
            other => panic!("expected the pull request's patch source, got {other:?}"),
        }
        assert_eq!(
            overlay.cwd,
            lyrel_core::core::path_spelling::local_spelling_buf(root.clone()),
            "the patch keeps the origin checkout"
        );
        let load = match &overlay.load {
            crate::ui::diff_overlay::DiffLoad::Ready(snapshot) => snapshot,
            _ => panic!("the supplied patch is installed, not probed"),
        };
        assert_eq!(load.files.len(), 1);
        assert_eq!(load.files[0].path, "src/lib.rs");
        assert!(
            load.files[0]
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .any(|line| line.text.contains("pr_supplied_marker")),
            "the visible patch is the fixture's supplied hunk"
        );
    });

    // Escape closes the patch and hands the keyboard back to the pull
    // request's detail document — not the shell underneath it.
    vcx.update(|window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("diff-overlay-header").is_none());
        assert_eq!(window.find("github-detail-document").focused(), Some(true));
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| app.github.open.clone()),
        Some(key),
        "closing the patch leaves the correct pull request in front"
    );

    // Escape again closes the detail and restores the covered document:
    // focus, reading position, and the unsaved edit, with nothing written
    // to or discarded from the file.
    vcx.update(|window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("github-detail-document").is_none());
    });
    assert_eq!(
        app.read_with(&vcx, |app, cx| app.editor.read(cx).had_focus()),
        tab,
        "input focus returns to the editor the review covered"
    );
    assert_eq!(
        editor.read_with(&vcx, |editor, _| editor.value().to_string()),
        "hello!"
    );
    assert_eq!(
        app.read_with(&vcx, |app, cx| {
            app.active_editor_input(cx)
                .and_then(|input| input.text().map(|text| text.read(cx).cursor_position()))
        }),
        Some(gpui_kit::component::input::Position::new(0, 6)),
        "the reading position returns with the document"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "hello",
        "closing the read-only surfaces neither saved nor discarded the file"
    );

    // The same round trip, keyboard only, while the document column is
    // filling the window the way a narrow width leaves it.
    open_pr(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("panel-github-file", 0usize), cx);
    });
    app.update_in(&mut vcx, |app, _, cx| {
        app.set_document_layout(lyrel_core::core::config::DocumentLayout::Fill, cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert_eq!(window.find("github-detail-document").focused(), Some(true));
        window.press("escape", cx);
        window.render_frame(cx);
    });
    assert_eq!(
        app.read_with(&vcx, |app, cx| app.editor.read(cx).had_focus()),
        tab,
        "filled to the window, the round trip still lands in the covered editor"
    );
    assert_eq!(
        editor.read_with(&vcx, |editor, _| editor.value().to_string()),
        "hello!"
    );
    app.update_in(&mut vcx, |app, _, cx| {
        app.set_document_layout(lyrel_core::core::config::DocumentLayout::Dock, cx);
    });
}

/// With no document underneath, the round trip ends on the list item the
/// reader came from: closing the patch returns to the pull request's detail,
/// and closing that returns focus to the still-valid selected row.
#[gpui_kit::test]
fn a_pr_patch_round_trip_with_no_prior_document_returns_to_the_selected_item(
    cx: &mut TestAppContext,
) {
    let (_checkout, root, app, mut vcx, _pane) = detail_doc_panel(cx, Kind::Pulls);
    let key = app.read_with(&vcx, |app, _| {
        app.github
            .lists
            .keys()
            .next()
            .map(|q| (q.slug.clone(), 8))
            .expect("the settled list names its repository")
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-8", cx);
    });
    settle(&app, &mut vcx, "the pull request's detail", |app| {
        app.github
            .details
            .get(&key)
            .and_then(|d| d.detail.as_ref())
            .and_then(|d| d.files.as_ref())
            .is_some_and(|files| !files.is_empty())
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("panel-github-file", 0usize), cx);
        window.render_frame(cx);
        assert!(window.find("diff-overlay-header").visible());
        window.press("escape", cx);
        window.render_frame(cx);
        assert_eq!(window.find("github-detail-document").focused(), Some(true));
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("github-detail-document").is_none());
        assert_eq!(
            window.find("panel-github-rows-acme/widgets").focused(),
            Some(true),
            "with no prior document the list regains the keyboard"
        );
        assert_eq!(window.find("panel-github-item-8").selected(), Some(true));
    });
    test_window::quiesce(&mut vcx, Some(&root));
}

/// The pane moving to another checkout must not re-originate the patch: it
/// stays the pull request's own supplied patch, named by the checkout it was
/// opened from, and closing it still returns to that pull request.
#[gpui_kit::test]
fn a_pr_patch_keeps_its_originating_checkout_when_the_pane_moves_on(cx: &mut TestAppContext) {
    let (_checkout, root, app, mut vcx, mut pane) = detail_doc_panel(cx, Kind::Pulls);
    let key = app.read_with(&vcx, |app, _| {
        app.github
            .lists
            .keys()
            .next()
            .map(|q| (q.slug.clone(), 8))
            .expect("the settled list names its repository")
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-8", cx);
    });
    settle(&app, &mut vcx, "the pull request's detail", |app| {
        app.github
            .details
            .get(&key)
            .and_then(|d| d.detail.as_ref())
            .and_then(|d| d.files.as_ref())
            .is_some_and(|files| !files.is_empty())
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("panel-github-file", 0usize), cx);
    });

    // The context pane moves to a different checkout of the same repository
    // — the situation two worktrees of one project put the reader in.
    let temp_b = tempfile::tempdir().unwrap();
    let root_b = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(temp_b.path()).unwrap(),
    );
    git(&root_b, &["init", "--quiet"]);
    git(
        &root_b,
        &["remote", "add", "origin", "https://github.com/acme/widgets"],
    );
    DaemonMsg::Cwd(root_b.clone()).encode(&mut pane).unwrap();
    wait_for_remotes(&app, &mut vcx, &root_b, Some("acme/widgets"));

    let origin = lyrel_core::core::path_spelling::local_spelling_buf(root.clone());
    app.read_with(&vcx, |app, _| {
        let overlay = app.tabs[app.tabs.active_index()]
            .diff_overlay
            .as_ref()
            .expect("the pane move left the patch open");
        match &overlay.source {
            crate::ui::scm::diff::DiffSource::Patch { id, .. } => {
                assert_eq!(id, "acme/widgets#8");
            }
            other => panic!("expected the pull request's patch source, got {other:?}"),
        }
        assert_eq!(
            overlay.cwd, origin,
            "the patch keeps the checkout the item was opened from, not the pane's current one"
        );
        assert!(
            matches!(overlay.load, crate::ui::diff_overlay::DiffLoad::Ready(_)),
            "no probe re-read the new pane's repository into the pull request's patch"
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert_eq!(window.find("github-detail-document").focused(), Some(true));
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| app.github.open.clone()),
        Some(key),
        "closing the patch returns to the pull request it came from"
    );
    test_window::quiesce(&mut vcx, Some(&root_b));
}

/// Switching items while their reads are still in flight: the answer that
/// lands late must stay in its own cache entry and cannot replace the detail,
/// the identity or the focus of the item now on screen.
#[gpui_kit::test]
fn a_late_detail_response_cannot_replace_the_item_now_on_screen(cx: &mut TestAppContext) {
    let root = scratch("detail-late");
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &["remote", "add", "origin", "https://github.com/acme/widgets"],
    );
    let fake = Arc::new(LatchedFake::new());
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    let transport: Arc<dyn Transport> = fake.clone();
    app.update_in(&mut vcx, |app, _, cx| {
        app.github.connector = Some(Arc::new(move || Connection {
            transport: transport.clone(),
        }));
        app.github.kind = Kind::Pulls;
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::GitHub;
        cx.notify();
    });
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    settle(&app, &mut vcx, "the pull request list", |app| {
        app.github
            .lists
            .iter()
            .any(|(q, l)| q.slug.full() == "acme/widgets" && q.kind == Kind::Pulls && l.loaded)
    });
    let slug = app.read_with(&vcx, |app, _| {
        app.github
            .lists
            .keys()
            .next()
            .map(|q| q.slug.clone())
            .expect("the settled list names its repository")
    });

    // Open the pull request, then the issue, while both item reads are still
    // parked behind the latch; only then let the answers land.
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-8", cx);
    });
    assert!(
        app.read_with(&vcx, |app, _| app.github.open.is_some()),
        "opening does not wait on the network"
    );
    vcx.update(|window, cx| window.click(("panel-github-kind", 0usize), cx));
    settle(&app, &mut vcx, "the issue list", |app| {
        app.github
            .lists
            .iter()
            .any(|(q, list)| q.slug == slug && q.kind == Kind::Issues && list.loaded)
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-7", cx);
    });
    let key = (slug.clone(), 7);
    fake.release();
    settle(&app, &mut vcx, "the issue's detail", |app| {
        app.github
            .details
            .get(&key)
            .is_some_and(|d| d.detail.is_some())
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("github-detail-document").focused(), Some(true));
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| app.github.open.clone()),
        Some(key),
        "the late pull request response did not reopen or replace the issue on screen"
    );
    assert!(
        app.read_with(&vcx, |app, _| {
            app.github
                .details
                .get(&(slug.clone(), 8))
                .is_some_and(|d| d.detail.is_some())
        }),
        "the late answer landed in its own cache entry, where a later open can use it"
    );
    test_window::quiesce(&mut vcx, Some(&root));
    let _ = std::fs::remove_dir_all(&root);
}
