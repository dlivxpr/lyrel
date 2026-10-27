//! Explicit real-endpoint coverage; never touches the installed daemon.
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{Entity, TestAppContext, TestSupportExt as _, VisualTestContext};

use super::route::{ForwardOwnerKey, ForwardRoute};
use crate::daemon::protocol::{ClientMsg, DaemonMsg, ForwardStatus, ShellSpec, SshPhase};
use crate::terminal::PaneWorkspace;
use crate::ui::app::{LyrelApp, test_window::harness_with_tabs};

mod native_fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/native_ssh_graphics.rs"));
}

struct RestoreConfig(PathBuf);
impl Drop for RestoreConfig {
    fn drop(&mut self) {
        crate::core::config::set_config_dir(self.0.clone());
    }
}

struct NativePane(u64);
impl Drop for NativePane {
    fn drop(&mut self) {
        if let Ok(mut stream) = crate::daemon::transport::connect() {
            let _ = ClientMsg::Kill { pane_id: self.0 }.encode(&mut stream);
        }
    }
}

fn native_pane(fixture: &native_fixture::NativeSshFixture) -> NativePane {
    let mut stream = crate::daemon::transport::connect().unwrap();
    stream.set_read_timeout(Some(native_fixture::STREAM_WITHIN)).unwrap();
    ClientMsg::SpawnNativeSsh {
        cwd: None,
        size: native_fixture::win_size(),
        spec: Box::new(fixture.spec()),
    }.encode(&mut stream).unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => NativePane(pane_id),
        other => panic!("native SSH pane was not spawned: {other:?}"),
    };
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::SshStatus { phase: SshPhase::Connected } => break,
            DaemonMsg::SshStatus { phase: SshPhase::Failed { reason } } => panic!("SSH fixture failed: {reason}"),
            DaemonMsg::AuthPrompt { .. } => panic!("explicit key/pin fixture must not prompt"),
            DaemonMsg::Exited { .. } | DaemonMsg::Error(_) => panic!("native SSH fixture exited"),
            _ => {}
        }
    }
    ClientMsg::Detach.encode(&mut stream).unwrap();
    pane
}

/// A real Linux Node listener in a tracked disposable pane. The existing native
/// fixture owns both that pane and its uniquely named remote script directory.
fn remote_echo(fixture: &native_fixture::NativeSshFixture) -> u16 {
    let host = fixture.connect_control();
    let node = fixture.install_sender(&host).program;
    let shell = ShellSpec {
        program: node,
        args: vec!["-e".into(), "require('node:net').createServer(s=>s.pipe(s)).listen(0,'127.0.0.1',function(){console.log('FORWARD_READY:'+this.address().port)})".into()],
        args_are_lyrel_defaults: false,
    };
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Spawn {
        cwd: None,
        size: native_fixture::win_size(),
        shell: Some(shell),
        owner: None,
        workspace: None,
        restore: None,
        allow_remote_clipboard_write: false,
        await_claim: false,
    }.encode(&mut stream).unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => pane_id,
        other => panic!("remote echo pane was not spawned: {other:?}"),
    };
    fixture.track_pane(pane);
    let mut output = Vec::new();
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Output(bytes) => output.extend(bytes),
            DaemonMsg::Error(error) => panic!("remote echo failed: {error}"),
            DaemonMsg::Exited { .. } => panic!("remote echo exited before listening"),
            _ => {}
        }
        let text = String::from_utf8_lossy(&output);
        if let Some(port) = text.split_once("FORWARD_READY:")
            .and_then(|(_, tail)| tail.split_once('\n').map(|(port, _)| port)) {
            if let Ok(port) = port.trim().parse() {
                ClientMsg::Detach.encode(&mut stream).unwrap();
                return port;
            }
        }
    }
}

fn echo(port: u16) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("forward listener");
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(b"managed-forward-proof\n").unwrap();
    let mut reply = [0; 22];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"managed-forward-proof\n");
}

fn exercise(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    pane_id: u64,
    remote_port: u16,
    other: &ForwardRoute,
) {
    // A known collision proves that only the short form requests an alternate
    // local port, and the actual tunneled bytes prove the endpoint mapping.
    let occupied = TcpListener::bind(("127.0.0.1", remote_port)).unwrap();
    app.update_in(vcx, |app, window, cx| {
        app.toggle_managed_forward_form(pane_id, window, cx);
        app.loopback_panel.mf_target_port.update(cx, |input, cx| input.set_value(remote_port.to_string(), window, cx));
        app.add_managed_forward(pane_id, window, cx);
        assert!(!app.loopback_panel.is_open_for(pane_id));
        let added = app.loopback_panel.managed().first().expect("short form added a rule").clone();
        assert_ne!(added.bind_port, remote_port, "collision uses an OS-selected port");
        assert!(matches!(added.status, ForwardStatus::Listening));
        echo(added.bind_port);
        assert!(other.list().is_empty(), "the other registry owner was not mutated");
        app.remove_managed_forward(pane_id, added.id, cx);
        assert!(app.loopback_panel.managed().is_empty());
    });
    drop(occupied);

    // Use the advanced form for a stable bind port, then reject a replacement
    // on a genuinely occupied port and continue editing the restored identity.
    let available = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let bind_port = available.local_addr().unwrap().port();
    drop(available);
    let blocker = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let rejected_port = blocker.local_addr().unwrap().port();
    app.update_in(vcx, |app, window, cx| {
        app.toggle_managed_forward_form(pane_id, window, cx);
        app.loopback_panel.toggle_advanced();
        for (input, value) in [
            (&app.loopback_panel.mf_bind_port, bind_port.to_string()),
            (&app.loopback_panel.mf_target_host, "127.0.0.1".into()),
            (&app.loopback_panel.mf_target_port, remote_port.to_string()),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        app.add_managed_forward(pane_id, window, cx);
        let old = app.loopback_panel.managed().first().unwrap().clone();
        echo(old.bind_port);
        app.loopback_panel.edit(old.clone(), window, cx);
        app.loopback_panel.mf_bind_port.update(cx, |input, cx| input.set_value(rejected_port.to_string(), window, cx));
        app.loopback_panel.mf_description.update(cx, |input, cx| input.set_value("retained draft", window, cx));
        app.add_managed_forward(pane_id, window, cx);
        assert!(app.loopback_panel.is_open_for(pane_id));
        assert!(app.loopback_panel.mf_error.is_some());
        assert_eq!(app.loopback_panel.mf_bind_port.read(cx).value().as_str(), rejected_port.to_string());
        assert_eq!(app.loopback_panel.mf_description.read(cx).value().as_str(), "retained draft");
        let restored = app.loopback_panel.managed().first().unwrap();
        assert_ne!(restored.id, old.id, "rollback really creates a new registry identity");
        assert_eq!(restored.bind_port, bind_port, "advanced form never silently picks another port");
        assert_eq!(app.loopback_panel.mf_editing.as_ref().unwrap().id, restored.id);
        assert_eq!(app.loopback_panel.managed().len(), 1);
        echo(restored.bind_port);
    });
    drop(blocker);
    app.update_in(vcx, |app, window, cx| {
        app.add_managed_forward(pane_id, window, cx);
        assert!(!app.loopback_panel.is_open_for(pane_id));
        assert_eq!(app.loopback_panel.managed().len(), 1, "continued save removed the restored id");
        let saved = app.loopback_panel.managed().first().unwrap().clone();
        assert_eq!(saved.bind_port, rejected_port);
        assert_eq!(saved.description.as_deref(), Some("retained draft"));
        echo(saved.bind_port);
        app.set_managed_forward_enabled(pane_id, saved.id, false, window, cx);
        let off = app.loopback_panel.managed().first().unwrap().clone();
        assert!(!off.enabled);
        app.loopback_panel.edit(off, window, cx);
        app.loopback_panel.mf_description.update(cx, |input, cx| input.set_value("edited while off", window, cx));
        app.add_managed_forward(pane_id, window, cx);
        let edited = app.loopback_panel.managed().first().unwrap().clone();
        assert!(!edited.enabled, "saving a disabled rule does not enable it");
        let released = TcpListener::bind(("127.0.0.1", edited.bind_port)).expect("disabled rule releases its listener");
        drop(released);
        app.set_managed_forward_enabled(pane_id, edited.id, true, window, cx);
        let on = app.loopback_panel.managed().first().unwrap().clone();
        assert!(on.enabled);
        echo(on.bind_port);
        app.remove_managed_forward(pane_id, on.id, cx);
        assert!(app.loopback_panel.managed().is_empty());
    });
}

#[gpui_kit::test]
#[ignore = "requires explicit isolated native SSH-to-Linux fixture and same-commit local server; run alone"]
fn managed_forward_real_routes_reject_recover_and_continue_editing(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let restore = RestoreConfig(crate::core::config::config_dir_path().unwrap());
    let binary = PathBuf::from(std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit lyrel-server required"));
    let fixture = native_fixture::NativeSshFixture::start(&binary);
    crate::core::config::set_config_dir(fixture.local_config_dir());
    let echo_port = remote_echo(&fixture);
    let native = native_pane(&fixture);
    let spec = fixture.spec();
    let workspace = PaneWorkspace {
        workspace: crate::core::session::WorkspaceId::new(),
        target: crate::core::session::RemoteTarget::direct(&spec.user, &spec.host, spec.port),
        spec: Some(Box::new(spec)),
        label: None,
        resize_echo: false,
    };
    let pane_route = ForwardRoute::new(native.0, None);
    let workspace_route = ForwardRoute::new(1, Some(workspace.clone()));
    assert!(matches!(pane_route.owner_key(), ForwardOwnerKey::Pane(_)));
    assert!(matches!(workspace_route.owner_key(), ForwardOwnerKey::Workspace(_)));
    let leaf = app.read_with(&vcx, |app, _| app.tabs[0].pane.terminals()[0].clone());
    leaf.update(&mut vcx, |view, _| view.pane_id = native.0);
    exercise(&app, &mut vcx, native.0, echo_port, &workspace_route);
    leaf.update(&mut vcx, |view, _| {
        view.pane_id = 1;
        view.set_workspace(Some(workspace));
    });
    exercise(&app, &mut vcx, 1, echo_port, &pane_route);
    workspace_route.teardown();
    drop(native);
    drop(fixture);
    drop(restore);
}
