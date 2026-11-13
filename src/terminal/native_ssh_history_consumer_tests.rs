// Real pinned SSH/SFTP history seeding through the production TerminalView.
use super::*;
use gpui_kit::Entity;
use lyrel_core::host::{Host as _, remote::RemoteHost};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct RemoteHistoryFile(Arc<RemoteHost>, std::path::PathBuf);
impl Drop for RemoteHistoryFile {
    fn drop(&mut self) {
        let _ = self.0.remove(&self.1, false);
    }
}

fn history_pane(
    fixture: &crate::native_ssh_graphics_fixture::NativeSshFixture,
    shell: crate::daemon::protocol::ShellSpec,
    host: Arc<RemoteHost>,
    cx: &mut TestAppContext,
) -> Entity<TerminalView> {
    use crate::daemon::protocol::ClientMsg;
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Spawn {
        cwd: Some(fixture.remote_home().into()),
        size: crate::native_ssh_graphics_fixture::win_size(),
        shell: Some(shell),
        owner: None,
        workspace: None,
        restore: None,
        allow_remote_clipboard_write: false,
        await_claim: false,
    }.encode(&mut stream).unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => pane_id,
        other => panic!("real history PTY spawn failed: {other:?}"),
    };
    fixture.track_pane(pane);
    stream.set_read_timeout(None).unwrap();
    let terminal = RemoteTerminal::from_stream_with(stream, TermSize::new(80, 24), Vec::new(), crate::terminal::remote::PtySource::Raw).unwrap();
    let built = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    cx.add_window(move |window, cx| {
        let view = cx.new(|cx| {
            let mut view = TerminalView::with_terminal(terminal, pane, window, cx);
            view.host_id = host.id();
            view
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = built.borrow_mut().take().unwrap();
    view
}

#[gpui_kit::test]
#[ignore = "requires isolated pinned SSH/SFTP, same-commit server and real nested SSH key/pin; run alone"]
fn native_ssh_workspace_history_uses_remote_home_and_isolates_inner_hop(cx: &mut TestAppContext) {
    let binary = std::path::PathBuf::from(std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit server"));
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    crate::core::config::set_config_dir(fixture.local_config_dir());
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let host = fixture.connect_control();
    let path = host.join(&host.home(), ".bash_history");
    assert!(!host.exists(&path), "refuse to overwrite even a fixture's pre-existing history");
    host.write_file(&path, b"echo REMOTE_HOME_ONLY_185\n").unwrap();
    let _history = RemoteHistoryFile(host.clone(), path);
    std::fs::write(fixture.local_config_dir().join("history"), b"echo LOCAL_ONLY_185\n").unwrap();
    cx.update(|cx| crate::ui::remote_connect::HostLinks::insert(cx, host.clone(), host.home()));
    let shell = crate::daemon::protocol::ShellSpec {
        program: "/bin/cat".into(), args: Vec::new(), args_are_lyrel_defaults: false,
    };
    let direct = history_pane(&fixture, shell.clone(), host.clone(), cx);
    direct.update(cx, |view, cx| view.follow_history_scope(cx));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if direct.read_with(cx, |view, _| view.history.entries().iter().any(|entry| entry == "echo REMOTE_HOME_ONLY_185")) {
            break;
        }
        assert!(Instant::now() < deadline, "remote HOME history never reached the connected pane");
        std::thread::sleep(Duration::from_millis(10));
    }
    direct.update(cx, |view, cx| {
        assert!(!view.history.entries().iter().any(|entry| entry.contains("LOCAL_ONLY_185")), "local history leaked into native SSH");
        view.input.editor_mut().set("echo REMOTE_HOME_ONLY");
        view.handle_editor_key(&key("up"), false, cx);
        assert_eq!(view.input.editor().text(), "echo REMOTE_HOME_ONLY_185");
    });

    let mut nested_shell = fixture.nested_shell(shell);
    // Production intentionally excludes `ssh host command` from interactive
    // remote history scopes. Open a real login shell instead of remote cat.
    nested_shell.args.pop();
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let command = std::iter::once(&nested_shell.program).chain(nested_shell.args.iter())
        .map(|arg| quote(arg)).collect::<Vec<_>>().join(" ");
    // A real user starts ssh from their existing shell. Making ssh itself
    // the pane's root shell suppresses the daemon's foreground-child detector.
    let nested = history_pane(&fixture, crate::daemon::protocol::ShellSpec {
        program: "/bin/bash".into(), args: vec!["--noprofile".into(), "--norc".into(), "-i".into(), "-c".into(), format!("{command}; :")],
        args_are_lyrel_defaults: false,
    }, host.clone(), cx);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if nested.read_with(cx, |view, _| view.remote_context().is_some()) {
            break;
        }
        assert!(Instant::now() < deadline, "real inner SSH hop missing: {:?}; screen={}",
            host.pane_procs(nested.read_with(cx, |view, _| view.pane_id)),
            nested.read_with(cx, |view, _| view.scrollback_text()));
        std::thread::sleep(Duration::from_millis(10));
    }
    nested.update(cx, |view, cx| view.follow_history_scope(cx));
    // Drain the actual asynchronous load before checking: an empty placeholder
    // cannot establish that the wrong remote file was not seeded later.
    cx.background_executor.run_until_parked();
    cx.run_until_parked();
    nested.update(cx, |view, cx| {
        assert!(!view.history.entries().iter().any(|entry| entry.contains("REMOTE_HOME_ONLY_185") || entry.contains("LOCAL_ONLY_185")), "outer/local history leaked into the real inner hop");
        view.handle_editor_key(&key("up"), false, cx);
        assert_eq!(view.input.editor().text(), "", "inner hop must not recall another Host's command");
    });
}

struct OwnedNativeHistoryPane(std::path::PathBuf, u64);
impl Drop for OwnedNativeHistoryPane {
    fn drop(&mut self) {
        let _ = lyrel_core::client::PaneClient::at(self.0.clone()).kill(self.1);
    }
}

#[gpui_kit::test]
#[ignore = "requires isolated pinned SSH/SFTP and same-commit local server; run alone"]
fn native_ssh_connected_shell_seeds_remote_history(cx: &mut TestAppContext) {
    use crate::daemon::protocol::{ClientMsg, RemoteKind, SshPhase};
    let binary = std::path::PathBuf::from(std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit server"));
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    crate::core::config::set_config_dir(fixture.local_config_dir());
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let host = fixture.connect_control();
    let path = host.join(&host.home(), ".bash_history");
    assert!(!host.exists(&path), "refuse to overwrite pre-existing fixture history");
    host.write_file(&path, b"echo LYREL185_NATIVE_HISTORY_SEED\n").unwrap();
    let _history = RemoteHistoryFile(host, path);
    let endpoint = fixture.local_config_dir().join(if cfg!(windows) { "daemon.port" } else { "daemon.sock" });
    let mut stream = crate::daemon::transport::connect_endpoint_at(&endpoint).unwrap();
    crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
    let mut spec = fixture.spec();
    spec.shell_integration = true;
    ClientMsg::SpawnNativeSsh {
        cwd: Some(fixture.remote_home().into()),
        size: crate::native_ssh_graphics_fixture::win_size(),
        spec: Box::new(spec),
    }.encode(&mut stream).unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => pane_id,
        other => panic!("real native SSH shell-channel spawn failed: {other:?}"),
    };
    let _pane = OwnedNativeHistoryPane(endpoint, pane);
    let terminal = RemoteTerminal::from_stream_with(stream, TermSize::new(80, 24), Vec::new(), crate::terminal::remote::PtySource::Raw).unwrap();
    let built = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    cx.add_window(move |window, cx| {
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = built.borrow_mut().take().unwrap();
    let connected_by = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        let connected = view.read_with(cx, |view, _| {
            assert!(view.host_id.is_local());
            let phase = view.terminal.ssh_phase();
            assert!(!matches!(phase, Some(SshPhase::Failed { .. })), "native shell fixture failed: {phase:?}");
            matches!(phase, Some(SshPhase::Connected))
                && view.remote_context().is_some_and(|context| context.kind == RemoteKind::NativeSsh)
                && view.input_active()
        });
        if connected { break; }
        assert!(Instant::now() < connected_by, "native SSH fixture never established a connected shell-channel pane with an active prompt editor");
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!("native SSH shell-channel connected; asserting SFTP history seed for pane {pane}");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        let seeded = view.update(cx, |view, cx| {
            assert!(view.host_id.is_local(), "the original native shell pane is not a remote-daemon workspace");
            view.follow_history_scope(cx);
            view.history.entries().iter().any(|entry| entry == "echo LYREL185_NATIVE_HISTORY_SEED")
        });
        if seeded { break; }
        assert!(Instant::now() < deadline, "connected native shell never seeded the far-end history through its own SFTP connection");
        std::thread::sleep(Duration::from_millis(10));
    }
    view.update(cx, |view, cx| {
        view.input.editor_mut().set("echo LYREL185_NATIVE_HISTORY");
        view.handle_editor_key(&key("up"), false, cx);
        assert_eq!(view.input.editor().text(), "echo LYREL185_NATIVE_HISTORY_SEED");
    });
}
