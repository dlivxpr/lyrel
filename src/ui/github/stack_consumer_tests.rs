// Consumers exercise the real API decoder, cache refresh, Root, rows and input
// dispatch. No copied graph helper or second data pipeline is used here.

#[gpui_kit::test]
fn stacked_pull_relationship_boundaries_are_deterministic_in_the_panel(cx: &mut TestAppContext) {
    let mut missing = stack_pull(15, "unknown", "a", "acme/widgets");
    missing["head"]["repo"] = serde_json::Value::Null;
    let fake = Arc::new(StackFake {
        pulls: Mutex::new(vec![
            stack_pull(1, "a", "main", "ACME/Widgets"),
            stack_pull(9, "solo", "main", "acme/widgets"),
            stack_pull(3, "c", "b", "acme/widgets"),
            stack_pull(2, "b", "a", "acme/widgets"),
            stack_pull(4, "sibling", "a", "acme/widgets"),
            stack_pull(11, "fork-head", "main", "someone/widgets"),
            stack_pull(12, "local", "fork-head", "acme/widgets"),
            stack_pull(13, "other-repository", "a", "acme/another"),
            stack_pull(14, "fork-child", "a", "someone/widgets"),
            missing,
            stack_pull(20, "loop-a", "loop-b", "acme/widgets"),
            stack_pull(21, "loop-b", "loop-a", "acme/widgets"),
            stack_pull(22, "self", "self", "acme/widgets"),
        ]),
    });
    let (temp, _app, mut vcx, _pane) = stack_panel(cx, fake);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let order = [4, 3, 2, 1, 9, 11, 12, 13, 14, 15, 21, 20, 22];
        for pair in order.windows(2) {
            let before = window
                .find(format!("panel-github-item-{}", pair[0]))
                .bounds();
            let after = window
                .find(format!("panel-github-item-{}", pair[1]))
                .bounds();
            assert!(
                before.top() < after.top(),
                "expected {} before {}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(window.find("panel-github-trunk-1").label(), Some("main"));
        for number in [9, 11, 12, 13, 14, 15, 20, 21, 22] {
            assert!(
                window
                    .try_find(format!("panel-github-trunk-{number}"))
                    .is_none()
            );
        }
        assert!(
            window.find("panel-github-stack-1").bounds().size.width
                > window.find("panel-github-stack-9").bounds().size.width,
            "siblings need separate connector lanes, not a misleading linear chain"
        );
    });
    test_window::quiesce(&mut vcx, Some(temp.path()));
}

#[gpui_kit::test]
fn stacked_pull_keyboard_keeps_identity_across_regroup_refresh_and_removal(
    cx: &mut TestAppContext,
) {
    let fake = Arc::new(StackFake {
        pulls: Mutex::new(vec![
            stack_pull(1, "a", "main", "acme/widgets"),
            stack_pull(3, "c", "b", "acme/widgets"),
            stack_pull(2, "b", "a", "acme/widgets"),
        ]),
    });
    let (temp, app, mut vcx, _pane) = stack_panel(cx, fake.clone());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-open-repo", cx);
        window.click_at(
            "panel-github-rows-acme/widgets",
            gpui_kit::point(gpui_kit::px(1.), gpui_kit::px(1.)),
            cx,
        );
        window.press("down", cx);
        assert_eq!(window.find("panel-github-item-3").selected(), Some(true));
        window.press("down", cx);
        assert_eq!(window.find("panel-github-item-2").selected(), Some(true));
    });
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/acme/widgets")
    );

    // Retargeting #2 to main splits its old stack. A new item also takes the
    // first source position: neither a source index nor a display index is #2.
    *fake.pulls.lock().unwrap() = vec![
        stack_pull(99, "new", "main", "acme/widgets"),
        stack_pull(1, "a", "main", "acme/widgets"),
        stack_pull(3, "c", "b", "acme/widgets"),
        stack_pull(2, "b", "main", "acme/widgets"),
    ];
    vcx.update(|window, cx| window.click("panel-github-refresh", cx));
    settle(&app, &mut vcx, "regrouped refresh", |app| {
        app.github
            .lists
            .values()
            .any(|list| list.items.first().is_some_and(|i| i.number == 99))
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("panel-github-item-2").selected(), Some(true));
        assert_eq!(window.find("panel-github-item-1").selected(), Some(false));
        window.click_at(
            "panel-github-rows-acme/widgets",
            gpui_kit::point(gpui_kit::px(1.), gpui_kit::px(1.)),
            cx,
        );
        window.press("enter", cx);
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| app.github.open.as_ref().map(|(_, n)| *n)),
        Some(2)
    );
    settle(&app, &mut vcx, "selected pull detail", |app| {
        app.github
            .details
            .iter()
            .any(|((_, n), detail)| *n == 2 && detail.detail.is_some())
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("panel-github-open-item", cx);
        window.click("panel-github-back", cx);
    });
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/acme/widgets/pull/2")
    );
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("panel-github-item-2").selected(), Some(true));
        window.press("up", cx);
        assert_eq!(
            window.find("panel-github-item-3").selected(),
            Some(true),
            "Back returns keyboard focus to the current list"
        );
        window.press("down", cx);
        assert_eq!(window.find("panel-github-item-2").selected(), Some(true));
    });

    fake.pulls
        .lock()
        .unwrap()
        .retain(|pull| pull["number"].as_u64() != Some(2));
    vcx.update(|window, cx| window.click("panel-github-refresh", cx));
    settle(&app, &mut vcx, "selected PR removed", |app| {
        app.github
            .lists
            .values()
            .all(|list| !list.items.iter().any(|i| i.number == 2))
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("panel-github-item-2").is_none());
        window.click_at(
            "panel-github-rows-acme/widgets",
            gpui_kit::point(gpui_kit::px(1.), gpui_kit::px(1.)),
            cx,
        );
        window.press("enter", cx);
    });
    assert!(
        app.read_with(&vcx, |app, _| app.github.open.is_none()),
        "Enter must not open an obsolete row position"
    );
    vcx.update(|window, cx| {
        window.press("end", cx);
        assert_eq!(window.find("panel-github-item-3").selected(), Some(true));
        window.press("home", cx);
        assert_eq!(window.find("panel-github-item-99").selected(), Some(true));
    });
    test_window::quiesce(&mut vcx, Some(temp.path()));
}

#[gpui_kit::test]
fn stacked_pull_label_filter_retains_flat_presentation_and_existing_links(cx: &mut TestAppContext) {
    let fake = Arc::new(StackFake {
        pulls: Mutex::new(vec![
            stack_pull(1, "a", "main", "acme/widgets"),
            stack_pull(3, "c", "b", "acme/widgets"),
            stack_pull(2, "b", "a", "acme/widgets"),
        ]),
    });
    let (temp, app, mut vcx, _pane) = stack_panel(cx, fake);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.hover("panel-github-item-1", cx);
        window.click("panel-github-label-1-0", cx);
    });
    settle(&app, &mut vcx, "label-filtered pulls", |app| {
        app.github
            .lists
            .iter()
            .any(|(q, l)| q.label.as_deref() == Some("stack") && l.loaded)
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let y = |n| window.find(format!("panel-github-item-{n}")).bounds().top();
        assert!(
            y(1) < y(3) && y(3) < y(2),
            "the filtered issues endpoint has no branch metadata"
        );
        assert!(window.try_find("panel-github-trunk-1").is_none());
        assert!(window.find("panel-github-open-repo").visible());
        assert!(
            window.try_find("panel-github-open-item").is_none(),
            "no duplicate detail link in the list"
        );
        window.click("panel-github-clear-label", cx);
        assert!(window.find("panel-github-trunk-1").visible());
        window.click("panel-github-item-2", cx);
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| app.github.open.as_ref().map(|(_, n)| *n)),
        Some(2),
        "pointer opening also uses PR identity"
    );
    test_window::quiesce(&mut vcx, Some(temp.path()));
}

#[gpui_kit::test]
fn stacked_pull_uses_the_canonical_api_repository_not_an_old_remote_name(cx: &mut TestAppContext) {
    let mut pulls = vec![
        stack_pull(1, "a", "main", "acme/renamed"),
        stack_pull(2, "b", "a", "acme/renamed"),
    ];
    for pull in &mut pulls {
        pull["base"]["repo"]["full_name"] = serde_json::json!("acme/renamed");
    }
    let fake = Arc::new(StackFake {
        pulls: Mutex::new(pulls),
    });
    let (temp, _app, mut vcx, _pane) = stack_panel(cx, fake);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find("panel-github-item-2").bounds().top()
                < window.find("panel-github-item-1").bounds().top()
        );
        assert_eq!(window.find("panel-github-trunk-1").label(), Some("main"));
    });
    test_window::quiesce(&mut vcx, Some(temp.path()));
}
