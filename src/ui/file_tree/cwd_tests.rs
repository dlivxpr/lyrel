use super::*;
use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WorkspaceId, WorkspaceStore};
use crate::terminal::PaneWorkspace;
use crate::terminal::view::quiet_test_pane;
use crate::ui::app::test_window::harness_with_pane;
use crate::ui::pane::{Pane, PaneSlot};
use gpui_kit::{Focusable as _, TestAppContext};
use lyrel_core::daemon::control::ControlHello;
use lyrel_core::host::remote::RemoteHost;
use std::net::{TcpListener, TcpStream};

#[gpui_kit::test]
fn remote_browsing_follows_the_foreground_cwd_not_repository_roots(cx: &mut TestAppContext) {
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    let target = RemoteTarget::Alias {
        alias: "cwd-consumer-test".into(),
    };
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    let thread = std::thread::spawn(move || {
        lyrel_core::host::server::serve(server, lyrel_core::host::local::LocalHost::new()).unwrap();
    });
    let host = RemoteHost::over_tcp(
        stream,
        &target.connection_key(),
        &ControlHello::host_rpc("cwd-consumer-test", "test-window"),
    )
    .unwrap();
    let id = target.host_id();
    let remote_workspace = WorkspaceId::new();
    let pane_workspace = PaneWorkspace {
        workspace: remote_workspace,
        target: target.clone(),
        spec: None,
        label: None,
        resize_echo: false,
    };
    let view = WindowView::on_remote(RemoteRef::new(target, remote_workspace));
    let window_workspace = view.id;
    let (left, right, _right_stream) = app.update_in(&mut vcx, |app, window, cx| {
        let mut views = WorkspaceStore::all(cx).clone();
        views.views.push(view);
        WorkspaceStore::install_for_test(cx, views);
        app.workspace = window_workspace;
        HostRegistry::insert(cx, host.clone());
        let left = app.tabs[0]
            .pane
            .first_leaf()
            .unwrap()
            .terminal()
            .unwrap()
            .clone();
        let (right, stream) = quiet_test_pane(2, window, cx);
        for pane in [&left, &right] {
            pane.update(cx, |pane, _| {
                pane.set_workspace(Some(pane_workspace.clone()))
            });
        }
        app.tabs[0].pane = Pane::split_node(
            gpui_kit::Axis::Horizontal,
            0.5,
            Pane::leaf(PaneSlot::Ready(left.clone())),
            Pane::leaf(PaneSlot::Ready(right.clone())),
        );
        app.file_tree.repo_roots.insert(
            id,
            PathBuf::from("/srv/repo/src"),
            PathBuf::from("/srv/repo"),
        );
        app.file_tree.repo_roots.insert(
            id,
            PathBuf::from("/srv/repo/tests"),
            PathBuf::from("/srv/repo"),
        );
        app.file_tree.repo_roots.insert(
            id,
            PathBuf::from("/tmp/outside"),
            PathBuf::from("/tmp/outside"),
        );
        left.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(PathBuf::from("/srv/repo/src")))
        });
        right.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(PathBuf::from("/srv/repo/tests")))
        });
        (left, right, stream)
    });
    app.update_in(&mut vcx, |app, window, cx| {
        for (pane, cwd) in [
            (&left, "/srv/repo/src"),
            (&right, "/srv/repo/tests"),
            (&left, "/tmp/outside"),
        ] {
            pane.update(cx, |pane, _| {
                pane.set_git_status_cwd_for_test(Some(PathBuf::from(cwd)))
            });
            window.focus(&pane.focus_handle(cx), cx);
            app.file_tree_refresh_roots(window, cx);
            assert_eq!(app.tab_code().unwrap().roots, vec![PathBuf::from(cwd)]);
            assert_eq!(
                app.tab_code().unwrap().roots[0].to_string_lossy(),
                cwd,
                "POSIX spelling must survive Windows"
            );
        }
        let (_, roots) = app.project_roots(cx).unwrap();
        assert!(
            roots.contains(&PathBuf::from("/srv/repo")),
            "search still uses repository roots"
        );
        assert!(roots.contains(&PathBuf::from("/tmp/outside")));
        app.sftp_close_browser(cx);
    });
    host.client().close();
    thread.join().unwrap();
}
