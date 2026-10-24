use super::*;
use crate::ui::app::test_window::{harness, harness_with_tabs};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

fn isolated(cx: &mut VisualTestContext) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|_, cx| install(cx, dir.path().join("notifications.json")));
    dir
}

fn messages(cx: &VisualTestContext) -> Vec<String> {
    cx.read(|cx| {
        history(cx)
            .read(cx)
            .records()
            .map(|record| record.message().to_string())
            .collect()
    })
}

fn restart(cx: &mut VisualTestContext, dir: &tempfile::TempDir) {
    cx.update(|_, cx| {
        history(cx).read(cx).flush();
        install(cx, dir.path().join("notifications.json"));
    });
}

#[gpui_kit::test]
fn all_toast_severities_are_received_with_content_and_persisted_unread(cx: &mut TestAppContext) {
    let (_, mut window) = harness(cx);
    let dir = isolated(&mut window);
    window.update(|window, cx| {
        window.notify_app(Notification::success("saved").title("project"), cx);
        window.notify_app(Notification::info("connected"), cx);
        window.notify_app(Notification::warning("disk low"), cx);
        window.notify_app(Notification::error("permission denied"), cx);
    });
    settle(&mut window);
    window.read(|cx| {
        let inbox = history(cx).read(cx);
        let received: Vec<_> = inbox
            .records()
            .map(|r| (r.severity(), r.message(), r.is_unread()))
            .collect();
        assert_eq!(
            received,
            vec![
                (Severity::Success, "project — saved", true),
                (Severity::Info, "connected", true),
                (Severity::Warning, "disk low", true),
                (Severity::Error, "permission denied", true),
            ]
        );
        assert!(inbox.records().all(|r| r.timestamp() > 1_700_000_000));
    });
    restart(&mut window, &dir);
    assert_eq!(
        messages(&window),
        [
            "project — saved",
            "connected",
            "disk low",
            "permission denied"
        ]
    );
    assert_eq!(window.read(|cx| history(cx).read(cx).unread_count()), 4);
}

#[gpui_kit::test]
fn two_windows_share_workspaces_read_state_clear_and_reopen(cx: &mut TestAppContext) {
    let (first_app, mut first) = harness(cx);
    let dir = isolated(&mut first);
    let (second_app, mut second) = harness(cx);
    let first_workspace = first_app.read_with(&first, |app, _| app.workspace);
    let second_workspace = second_app.read_with(&second, |app, _| app.workspace);
    assert_ne!(first_workspace, second_workspace);
    first.update(|window, cx| window.notify_app("first workspace", cx));
    second.update(|window, cx| window.notify_app("second workspace", cx));
    settle(&mut first);
    settle(&mut second);
    assert_eq!(messages(&first), ["first workspace", "second workspace"]);
    assert_eq!(messages(&second), ["first workspace", "second workspace"]);
    first.read(|cx| {
        let sources: Vec<_> = history(cx)
            .read(cx)
            .records()
            .map(|r| r.source().unwrap().workspace)
            .collect();
        assert_eq!(sources, [first_workspace, second_workspace]);
    });
    first.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut first);
    assert_eq!(second.read(|cx| history(cx).read(cx).unread_count()), 0);
    restart(&mut second, &dir);
    assert_eq!(second.read(|cx| history(cx).read(cx).unread_count()), 0);
    // A new event after reading is unread in both windows.
    second.update(|window, cx| window.notify_app("later", cx));
    settle(&mut second);
    assert_eq!(first.read(|cx| history(cx).read(cx).unread_count()), 1);
    first.update(|window, cx| window.click("notification-clear-history", cx));
    settle(&mut first);
    assert!(messages(&second).is_empty());
    assert_eq!(second.read(|cx| history(cx).read(cx).unread_count()), 0);
    // Clear has no confirmation and generates no success notification.
    first.update(|window, _| assert!(window.find("notification-inbox-empty").visible()));
    restart(&mut second, &dir);
    assert!(messages(&second).is_empty());
    first.simulate_keystrokes("escape");
    settle(&mut first);
}

#[gpui_kit::test]
fn newest_five_hundred_survive_overflow_clear_then_append_and_restart(cx: &mut TestAppContext) {
    let (_, mut window) = harness(cx);
    let dir = isolated(&mut window);
    window.update(|window, cx| {
        for n in 1..=500 {
            window.notify_app(format!("event {n}"), cx);
        }
    });
    settle(&mut window);
    let retained = messages(&window);
    assert_eq!(retained.len(), 500);
    assert_eq!(retained.first().unwrap(), "event 1");
    assert_eq!(retained.last().unwrap(), "event 500");
    window.update(|window, cx| window.notify_app("event 501", cx));
    settle(&mut window);
    let retained = messages(&window);
    assert_eq!(retained.len(), 500);
    assert_eq!(retained.first().unwrap(), "event 2");
    assert_eq!(retained.last().unwrap(), "event 501");
    restart(&mut window, &dir);
    assert_eq!(messages(&window), retained);
    window.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut window);
    window.update(|window, cx| {
        window.click("notification-clear-history", cx);
        window.notify_app("after clear", cx);
    });
    settle(&mut window);
    restart(&mut window, &dir);
    assert_eq!(messages(&window), ["after clear"]);
    assert_eq!(window.read(|cx| history(cx).read(cx).unread_count()), 1);
}

#[gpui_kit::test]
fn clearing_an_empty_inbox_is_immediate_and_does_not_disable_later_events(cx: &mut TestAppContext) {
    let (_, mut window) = harness(cx);
    let dir = isolated(&mut window);
    settle(&mut window);
    window.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut window);
    window.update(|window, cx| {
        assert!(window.find("notification-inbox-empty").visible());
        window.click("notification-clear-history", cx);
    });
    settle(&mut window);
    restart(&mut window, &dir);
    assert!(messages(&window).is_empty());
    window.update(|window, cx| window.notify_app(Notification::warning("real later event"), cx));
    settle(&mut window);
    restart(&mut window, &dir);
    assert_eq!(messages(&window), ["real later event"]);
}

#[gpui_kit::test]
fn retained_source_navigates_but_removed_source_never_recreates_a_session(cx: &mut TestAppContext) {
    let (app, mut window, _streams) = harness_with_tabs(cx, 2);
    let _dir = isolated(&mut window);
    app.update_in(&mut window, |app, window, cx| {
        let source = app.notification_source(window, cx);
        window.notify_app(Notification::info("from first tab").source(source), cx);
        app.active = 1;
        cx.notify();
    });
    settle(&mut window);
    window.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut window);
    let record = window.read(|cx| history(cx).read(cx).records().next().unwrap().clone());
    window.update(|window, cx| window.click(format!("notification-source-{}", record.id()), cx));
    settle(&mut window);
    assert_eq!(app.read_with(&window, |app, _| app.active), 0);
    app.update_in(&mut window, |app, _, cx| {
        app.tabs.remove(0);
        app.active = 0;
        cx.notify();
    });
    settle(&mut window);
    window.update(|window, cx| {
        assert!(!source_exists(record.source().unwrap(), window, cx));
        assert!(!navigate(record.source().unwrap(), window, cx));
    });
    assert_eq!(app.read_with(&window, |app, _| app.tabs.len()), 1);
    assert_eq!(messages(&window), ["from first tab"]);
    assert_eq!(
        window.read(|cx| history(cx)
            .read(cx)
            .records()
            .next()
            .unwrap()
            .source()
            .cloned()),
        record.source().cloned()
    );
}

#[gpui_kit::test]
fn a_fresh_application_restores_received_records_read_state_and_clear(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut first_context = cx.new_app();
        let (app, mut first) = harness(&mut first_context);
        first.update(|window, cx| {
            install(cx, dir.path().join("notifications.json"));
            window.notify_app(Notification::error("received before GUI exit"), cx);
        });
        settle(&mut first);
        first.read(|cx| history(cx).read(cx).flush());
        drop(first);
        drop(app);
        first_context.quit();
    }
    {
        let mut reopened_context = cx.new_app();
        let (app, mut reopened) = harness(&mut reopened_context);
        reopened.update(|_, cx| install(cx, dir.path().join("notifications.json")));
        assert_eq!(messages(&reopened), ["received before GUI exit"]);
        assert_eq!(reopened.read(|cx| history(cx).read(cx).unread_count()), 1);
        settle(&mut reopened);
        reopened.update(|window, cx| window.click("status-notifications", cx));
        settle(&mut reopened);
        assert_eq!(reopened.read(|cx| history(cx).read(cx).unread_count()), 0);
        reopened.read(|cx| history(cx).read(cx).flush());
        drop(reopened);
        drop(app);
        reopened_context.quit();
    }
    let (_, mut final_window) = harness(cx);
    final_window.update(|_, cx| install(cx, dir.path().join("notifications.json")));
    assert_eq!(messages(&final_window), ["received before GUI exit"]);
    assert_eq!(
        final_window.read(|cx| history(cx).read(cx).unread_count()),
        0
    );
    settle(&mut final_window);
    final_window.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut final_window);
    final_window.update(|window, cx| window.click("notification-clear-history", cx));
    settle(&mut final_window);
    final_window.read(|cx| history(cx).read(cx).flush());
    let mut after_clear_context = cx.new_app();
    let (_, mut after_clear) = harness(&mut after_clear_context);
    after_clear.update(|_, cx| install(cx, dir.path().join("notifications.json")));
    assert!(messages(&after_clear).is_empty());
    assert_eq!(
        after_clear.read(|cx| history(cx).read(cx).unread_count()),
        0
    );
}

#[gpui_kit::test]
fn a_settings_window_error_uses_the_same_inbox_and_preserves_its_workspace_source(
    cx: &mut TestAppContext,
) {
    let (app, mut main) = harness(cx);
    let _dir = isolated(&mut main);
    main.dispatch_action(crate::core::actions::OpenSettings);
    settle(&mut main);
    let owner = main.update(|window, _| window.window_handle());
    let settings = cx.add_window(|window, cx| {
        app.update(cx, |app, _| {
            app.settings_window = Some((window.window_handle(), owner));
        });
        let view = cx.new(|cx| crate::ui::settings_window::SettingsWindow::new(&app, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    cx.update_window(settings.into(), |_, window, cx| {
        crate::ui::host_ops::HostOps::notify_err(
            window,
            cx,
            "settings save failed",
            &std::io::Error::new(std::io::ErrorKind::PermissionDenied, "settings fixture"),
        );
    })
    .unwrap();
    settle(&mut main);
    let workspace = app.read_with(&main, |app, _| app.workspace);
    main.read(|cx| {
        let inbox = history(cx).read(cx);
        let record = inbox.records().next().unwrap();
        assert_eq!(record.severity(), Severity::Error);
        assert!(record.message().contains("settings save failed"));
        assert_eq!(record.source().unwrap().workspace, workspace);
        assert_eq!(inbox.unread_count(), 1);
    });
}

#[gpui_kit::test]
fn source_navigation_selects_the_other_window_without_touching_its_other_tabs(
    cx: &mut TestAppContext,
) {
    let (first_app, mut first, _first_streams) = harness_with_tabs(cx, 2);
    let _dir = isolated(&mut first);
    let (second_app, mut second, _second_streams) = harness_with_tabs(cx, 2);
    first.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        let workspace = first_app.read(cx).workspace;
        crate::ui::windows::WindowRegistry::register(
            cx,
            workspace,
            window.window_handle(),
            first_app.downgrade(),
        );
    });
    second.update(|window, cx| {
        let workspace = second_app.read(cx).workspace;
        crate::ui::windows::WindowRegistry::register(
            cx,
            workspace,
            window.window_handle(),
            second_app.downgrade(),
        );
    });
    second_app.update_in(&mut second, |app, window, cx| {
        let source = app.notification_source(window, cx);
        window.notify_app(
            Notification::info("other workspace first pane").source(source),
            cx,
        );
        app.active = 1;
        cx.notify();
    });
    settle(&mut first);
    settle(&mut second);
    first.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut first);
    let id = first.read(|cx| history(cx).read(cx).records().next().unwrap().id());
    first.update(|window, cx| window.click(format!("notification-source-{id}"), cx));
    settle(&mut first);
    settle(&mut second);
    assert_eq!(second_app.read_with(&second, |app, _| app.active), 0);
    assert_eq!(second_app.read_with(&second, |app, _| app.tabs.len()), 2);
    assert_eq!(first_app.read_with(&first, |app, _| app.tabs.len()), 2);
    assert_eq!(messages(&second), ["other workspace first pane"]);
}

#[gpui_kit::test]
fn parked_source_opens_its_preview_without_restoring_or_focusing_the_hidden_shell(
    cx: &mut TestAppContext,
) {
    let (app, mut window, _streams) = harness_with_tabs(cx, 2);
    let _dir = isolated(&mut window);
    window.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut window, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        let source = app.notification_source(window, cx);
        window.notify_app(
            Notification::info("parked task completed").source(source),
            cx,
        );
        pane
    });
    window.dispatch_action(crate::core::actions::ParkPane);
    settle(&mut window);
    app.update_in(&mut window, |app, _, cx| {
        app.active = 1;
        cx.notify();
    });
    settle(&mut window);
    window.update(|window, cx| window.click("status-notifications", cx));
    settle(&mut window);
    let id = window.read(|cx| history(cx).read(cx).records().next().unwrap().id());
    window.update(|window, cx| window.click(format!("notification-source-{id}"), cx));
    settle(&mut window);
    app.update_in(&mut window, |app, window, cx| {
        assert_eq!(app.active, 0);
        assert!(app.tabs[0].is_parked_slot(&pane, cx));
        assert!(!pane.contains_focused(window, cx));
    });
    window.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("parked-preview").visible());
        window.press("enter", cx);
    });
    settle(&mut window);
    app.update_in(&mut window, |app, window, cx| {
        assert!(!app.tabs[0].is_parked_slot(&pane, cx));
        assert!(pane.contains_focused(window, cx));
        assert_eq!(app.tabs.len(), 2);
    });
}

#[gpui_kit::test]
fn equal_pane_numbers_on_different_daemons_preserve_notification_destination(
    cx: &mut TestAppContext,
) {
    use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WindowViews};
    let (app, mut window, _remote_stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let dir = isolated(&mut window);
    let route = RemoteRef::new(
        RemoteTarget::direct("me", "notification-host", 22),
        WorkspaceId::new(),
    );
    let view = WindowView::on_remote(route.clone());
    let (remote, native, _native_stream) = app.update_in(&mut window, |app, window, cx| {
        app.workspace = view.id;
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![view],
                active: None,
            },
        );
        let remote = app.tabs[0].pane.first_leaf().unwrap();
        remote.terminal().unwrap().update(cx, |terminal, _| {
            terminal.set_workspace(Some(crate::terminal::PaneWorkspace {
                workspace: route.workspace,
                target: route.target.clone(),
                spec: None,
                label: None,
                resize_echo: false,
            }));
        });
        let (native, stream) = crate::terminal::view::quiet_test_ssh_pane(1, window, cx);
        app.tabs[0].pane.split_leaf(
            remote.entity_id(),
            gpui_kit::Axis::Horizontal,
            false,
            PaneSlot::Ready(native.clone()),
        );
        let focus = native.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        let source = app.notification_source(window, cx);
        window.notify_app(Notification::info("focused native pane").source(source), cx);
        window.notify_app(
            Notification::info("asynchronous native pane").pane(
                Some(app.workspace),
                1,
                &crate::terminal::PaneRoute::Local,
            ),
            cx,
        );
        (remote, native, stream)
    });
    settle(&mut window);
    restart(&mut window, &dir);
    let records: Vec<_> =
        window.read(|cx| history(cx).read(cx).records().map(Record::id).collect());
    for id in records {
        window.update(|window, cx| window.focus(&remote.focus_handle(cx), cx));
        window.update(|window, cx| window.click("status-notifications", cx));
        settle(&mut window);
        window.update(|window, cx| window.click(format!("notification-source-{id}"), cx));
        settle(&mut window);
        window.update(|window, cx| {
            assert!(
                native.read(cx).focus_handle.is_focused(window),
                "a different daemon's equal pane number must not steal navigation"
            );
            assert!(!remote.contains_focused(window, cx));
        });
    }
}

#[gpui_kit::test]
fn a_forgotten_remote_notification_cannot_reopen_as_a_local_workspace(cx: &mut TestAppContext) {
    use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WindowViews};
    use crate::ui::machine_mirror::MachineMirrors;
    use lyrel_core::core::machine::{Machine, Tab, Workspace};

    let (app, mut window) = harness(cx);
    let _dir = isolated(&mut window);
    let remote = RemoteRef::new(
        RemoteTarget::direct("me", "forgotten-notification-host", 22),
        WorkspaceId::new(),
    );
    let view = WindowView::on_remote(remote.clone());
    let tab = Tab::leaf(41);
    let source = Source {
        workspace: view.id,
        machine_workspace: remote.workspace,
        host: remote.host_id().0,
        tab: Some(tab.id),
        pane: Some(41),
        pane_owner: Some(PaneOwner::Workspace),
        label: "remote workspace · #41".into(),
    };
    window.update(|window, cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![view],
                active: None,
            },
        );
        MachineMirrors::install(
            cx,
            remote.host_id(),
            Machine {
                workspaces: vec![Workspace {
                    id: remote.workspace,
                    tabs: vec![tab],
                    ..Workspace::default()
                }],
                panes: Vec::new(),
            },
        );
        window.notify_app(
            Notification::info("remote finished").source(source.clone()),
            cx,
        );
        assert!(source_exists(&source, window, cx));
        WorkspaceStore::remove(cx, source.workspace);
        assert!(MachineMirrors::machine(cx, remote.host_id()).is_some());
        assert!(
            !source_exists(&source, window, cx),
            "a cached remote tree is not a current route to the forgotten workspace"
        );
        assert!(!navigate(&source, window, cx));
    });
    settle(&mut window);
    assert_eq!(messages(&window), ["remote finished"]);
    assert_ne!(
        app.read_with(&window, |app, _| app.workspace),
        source.workspace
    );
}
