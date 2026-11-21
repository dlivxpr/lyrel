use super::render_idle_gpui_tests::serial;
use super::*;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_pane;
use crate::ui::pane::{Pane, PaneSlot};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::daemon::control::ControlHello;
use lyrel_core::host::remote::RemoteHost;
use lyrel_terminal::view::{TerminalView, quiet_test_pane};
use lyrel_ui_base::route::PaneWorkspace;
use lyrel_ui_base::session::{RemoteRef, RemoteTarget, WindowView, WorkspaceId, WorkspaceStore};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// A real repository with a nested directory, a linked worktree with its own
/// nested directory, and a plain directory outside Git — the acceptance shape
/// for "the checkout root only, never the common Git directory".
struct Checkouts {
    // Held to keep the fixture directories alive for the whole test; never read.
    _base: tempfile::TempDir,
    nested_repo: PathBuf,
    repo_root: PathBuf,
    nested_worktree: PathBuf,
    worktree_root: PathBuf,
    plain: PathBuf,
}

fn checkouts(name: &str) -> Checkouts {
    let temp_root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(std::env::temp_dir()).unwrap(),
    );
    let temp = tempfile::tempdir_in(temp_root).unwrap_or_else(|e| panic!("checkouts base: {e}"));
    let base = temp.path().join(name);
    std::fs::create_dir_all(&base).unwrap();
    let repo_root = base.join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    let git = |args: &[&std::ffi::OsStr]| {
        let status = Command::new("git")
            .args(args)
            .status()
            .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} exited {status}");
    };
    git(&[
        std::ffi::OsStr::new("init"),
        std::ffi::OsStr::new("--quiet"),
        repo_root.as_os_str(),
    ]);
    git(&[
        std::ffi::OsStr::new("-C"),
        repo_root.as_os_str(),
        std::ffi::OsStr::new("-c"),
        std::ffi::OsStr::new("user.email=files@lyrel"),
        std::ffi::OsStr::new("-c"),
        std::ffi::OsStr::new("user.name=Files Test"),
        std::ffi::OsStr::new("commit"),
        std::ffi::OsStr::new("--allow-empty"),
        std::ffi::OsStr::new("--quiet"),
        std::ffi::OsStr::new("--message"),
        std::ffi::OsStr::new("init"),
    ]);
    let worktree_root = base.join("worktree");
    git(&[
        std::ffi::OsStr::new("-C"),
        repo_root.as_os_str(),
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("add"),
        std::ffi::OsStr::new("--quiet"),
        worktree_root.as_os_str(),
    ]);
    let nested_repo = repo_root.join("src");
    std::fs::create_dir_all(&nested_repo).unwrap();
    let nested_worktree = worktree_root.join("src");
    std::fs::create_dir_all(&nested_worktree).unwrap();
    let plain = base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    Checkouts {
        _base: temp,
        nested_repo,
        repo_root,
        nested_worktree,
        worktree_root,
        plain,
    }
}

/// Focus `pane` — making it the Context Pane — and wait until the Files tab's
/// tree and its content search both describe exactly `expected`: one checkout
/// root on the pane's owning Host, never a union of pane roots. The filename
/// search runs over the tree's own roots, so it shares the root by
/// construction.
fn assert_shared_root(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    pane: &Entity<TerminalView>,
    expected: &[PathBuf],
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        vcx.background_executor.run_until_parked();
        let rooted = app.update_in(vcx, |app, window, cx| {
            window.focus(&pane.focus_handle(cx), cx);
            app.file_tree_refresh_roots(window, cx);
            let tree_roots = app.tab_code().map(|c| c.roots.clone()).unwrap_or_default();
            let search_roots = app
                .project_roots(window, cx)
                .map(|(_, roots)| roots)
                .unwrap_or_default();
            tree_roots == expected && search_roots == expected
        });
        if rooted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the Files tab never described {expected:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui_kit::test]
fn local_files_tree_and_both_searches_share_the_context_pane_checkout(cx: &mut TestAppContext) {
    let _serial = serial();
    let fixture = checkouts("local");
    let (app, mut vcx, _stream) = harness_with_pane(cx);
    let (repo_pane, worktree_pane, _worktree_stream) =
        app.update_in(&mut vcx, |app, window, cx| {
            let repo_pane = app.tabs[0]
                .pane
                .first_leaf()
                .unwrap()
                .terminal()
                .unwrap()
                .clone();
            let (worktree_pane, worktree_stream) = quiet_test_pane(2, window, cx);
            app.tabs[0].pane = Pane::split_node(
                gpui_kit::Axis::Horizontal,
                0.5,
                Pane::leaf(PaneSlot::Ready(repo_pane.clone())),
                Pane::leaf(PaneSlot::Ready(worktree_pane.clone())),
            );
            repo_pane.update(cx, |pane, _| {
                pane.set_git_status_cwd_for_test(Some(fixture.nested_repo.clone()))
            });
            worktree_pane.update(cx, |pane, _| {
                pane.set_git_status_cwd_for_test(Some(fixture.nested_worktree.clone()))
            });
            (repo_pane.clone(), worktree_pane, worktree_stream)
        });
    // Both panes sit in a nested directory of a checkout, so both answers are
    // the checkout root of the focused one — a single root, never the nested
    // cwd, never a union of the two panes' roots, and for the linked
    // worktree never the common Git directory.
    assert_shared_root(&app, &mut vcx, &repo_pane, &[fixture.repo_root.clone()]);
    assert_shared_root(
        &app,
        &mut vcx,
        &worktree_pane,
        &[fixture.worktree_root.clone()],
    );
    // Outside Git the pane's own directory is the shared root.
    app.update_in(&mut vcx, |_, _, cx| {
        worktree_pane.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(fixture.plain.clone()));
        });
    });
    assert_shared_root(&app, &mut vcx, &worktree_pane, &[fixture.plain.clone()]);
}

#[gpui_kit::test]
fn remote_files_tree_and_both_searches_share_the_context_pane_checkout(cx: &mut TestAppContext) {
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
        let shared = app.active_host(cx).unwrap();
        app.file_tree
            .update(cx, |tree, cx| tree.sync_watch(shared, HashSet::new(), cx));
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
        app.file_tree.update(cx, |tree, _| {
            tree.repo_roots.insert(
                id,
                PathBuf::from("/srv/repo/src"),
                PathBuf::from("/srv/repo"),
            );
            tree.repo_roots.insert(
                id,
                PathBuf::from("/srv/repo/tests"),
                PathBuf::from("/srv/repo"),
            );
            tree.repo_roots.insert(
                id,
                PathBuf::from("/tmp/outside"),
                PathBuf::from("/tmp/outside"),
            );
        });
        left.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(PathBuf::from("/srv/repo/src")))
        });
        right.update(cx, |pane, _| {
            pane.set_git_status_cwd_for_test(Some(PathBuf::from("/srv/repo/tests")))
        });
        (left, right, stream)
    });
    app.update_in(&mut vcx, |app, window, cx| {
        for (pane, cwd, root) in [
            (&left, "/srv/repo/src", "/srv/repo"),
            (&right, "/srv/repo/tests", "/srv/repo"),
            (&left, "/tmp/outside", "/tmp/outside"),
        ] {
            pane.update(cx, |pane, _| {
                pane.set_git_status_cwd_for_test(Some(PathBuf::from(cwd)))
            });
            window.focus(&pane.focus_handle(cx), cx);
            app.file_tree_refresh_roots(window, cx);
            let expected = vec![PathBuf::from(root)];
            assert_eq!(
                app.tab_code().unwrap().roots,
                expected,
                "the tree roots itself at the context pane's checkout"
            );
            assert_eq!(
                expected[0].to_string_lossy(),
                root,
                "POSIX spelling must survive Windows"
            );
            // The content search reads the same single root — the tree and
            // both searches describe one checkout, on this Host.
            let (_, search_roots) = app
                .project_roots(window, cx)
                .expect("the checkout is cached; resolution is synchronous");
            assert_eq!(search_roots, expected);
        }
        app.sftp_close_browser(cx);
    });
    host.client().close();
    thread.join().unwrap();
}
