// The issue/PR detail as a document: opened from the list into the document
// area, read and dismissed with the keyboard, docked and filled like any
// other document, and closed without touching the file it covered.

/// A repository with the shared `Fake` fixture transport, the GitHub panel
/// open, and the list settled — the common half of every detail test.
fn detail_doc_panel(
    cx: &mut TestAppContext,
) -> (
    PathBuf,
    Entity<LyrelApp>,
    VisualTestContext,
    crate::daemon::transport::Stream,
) {
    let root = scratch("detail-document");
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
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::GitHub;
        cx.notify();
    });
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    settle(&app, &mut vcx, "the issue list", |app| {
        app.github
            .lists
            .iter()
            .any(|(q, l)| q.slug.full() == "acme/widgets" && l.loaded)
    });
    (root, app, vcx, pane)
}

#[gpui_kit::test]
fn enter_opens_the_detail_document_and_escape_returns_to_the_list(cx: &mut TestAppContext) {
    let (root, app, mut vcx, _pane) = detail_doc_panel(cx);
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
fn clicking_a_row_opens_the_detail_over_a_file_and_close_restores_it(
    cx: &mut TestAppContext,
) {
    let (root, app, mut vcx, _pane) = detail_doc_panel(cx);
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
    test_window::quiesce(&mut vcx, Some(&root));
}

#[gpui_kit::test]
fn the_detail_document_docks_and_fills_like_other_documents(cx: &mut TestAppContext) {
    let (root, _app, mut vcx, _pane) = detail_doc_panel(cx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-item-7", cx);
        window.render_frame(cx);
        // Docked beside the terminal, the document takes the column's width.
        let docked = window.find("github-detail-document").bounds();
        window.click(("document-layout", 1), cx);
        window.render_frame(cx);
        let filled = window.find("github-detail-document").bounds();
        assert!(
            filled.size.width > docked.size.width,
            "fill covers the workspace; the dock takes the column beside the terminal"
        );
        window.click(("document-layout", 0), cx);
        window.render_frame(cx);
        let again = window.find("github-detail-document").bounds();
        assert_eq!(
            again.size.width, docked.size.width,
            "asking for the dock again restores the requested split"
        );
    });
    test_window::quiesce(&mut vcx, Some(&root));
}
