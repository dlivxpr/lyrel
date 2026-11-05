// Real pinned SSH workspace ownership through the rooted workbench supervisor.
use super::*;
use gpui_kit::{TestAppContext, VisualContext as _};
use lyrel_core::host::Host as _;

fn wait_for_link(cx: &mut TestAppContext, host: HostId) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        cx.run_until_parked();
        if cx.update(|cx| {
            cx.try_global::<RemoteLinks>()
                .and_then(|links| links.machines.get(&host))
                .is_some_and(|link| !link.attempting && link.state == LinkState::Attached)
        }) {
            return;
        }
        assert!(Instant::now() < deadline, "real native SSH reconnect did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn attachment(host: &RemoteHost, workspace: WorkspaceId) -> lyrel_core::core::machine::Attachment {
    match host.client().call(ControlRequest::WorkspaceTree { workspace }).unwrap() {
        ReplyOk::WorkspaceTree(tree) => tree.attachment.clone().expect("workspace holder"),
        other => panic!("expected real workspace tree, got {other:?}"),
    }
}

struct OwnedWorkspace(Arc<RemoteHost>, WorkspaceId);
impl Drop for OwnedWorkspace {
    fn drop(&mut self) {
        let _ = self.0.client().call(ControlRequest::WorkspaceRemove { workspace: self.1 });
    }
}

#[gpui_kit::test]
#[ignore = "requires explicit isolated pinned native SSH/SFTP and same-commit server; run alone"]
fn native_ssh_workspace_reconnect_yields_to_active_client(cx: &mut TestAppContext) {
    let binary = PathBuf::from(std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit local server"));
    let mut fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    crate::core::config::set_config_dir(fixture.local_config_dir());
    let observer = fixture.connect_control();
    let remote_id = WorkspaceId::new();
    assert!(matches!(observer.client().call(ControlRequest::WorkspaceCreate {
        name: Some("isolated reconnect ownership".into()), workspace: Some(remote_id),
    }).unwrap(), ReplyOk::WorkspaceTree(_)));
    let mut owned = OwnedWorkspace(observer.clone(), remote_id);

    let (app, vcx) = crate::ui::app::test_window::harness(cx);
    let (other_window, other_vcx) = crate::ui::app::test_window::harness(cx);
    let spec = fixture.spec();
    let mut profile = crate::core::ssh_profile::SshProfile::new("isolated lifecycle");
    profile.host = spec.host;
    profile.port = spec.port;
    profile.user = spec.user;
    profile.identity_files = spec.identity_files;
    profile.auth = crate::core::ssh_profile::AuthMode::PublicKey;
    profile.verify_host_keys = Some(true);
    profile.shell_integration = false;
    let target = RemoteTarget::Profile { id: profile.id };
    let host = target.host_id();
    let workspace = app.read_with(cx, |app, _| app.workspace);
    let local_workspace = other_window.read_with(cx, |app, _| app.workspace);
    cx.update(|cx| {
        cx.global_mut::<Config>().ssh_profiles.push(profile);
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(cx, workspace, vcx.window_handle(), app.downgrade());
        crate::ui::windows::WindowRegistry::register(cx, local_workspace, other_vcx.window_handle(), other_window.downgrade());
        let mut remote = crate::core::session::WindowView::on_remote(RemoteRef::new(target.clone(), remote_id));
        remote.id = workspace;
        remote.open = true;
        let mut local = crate::core::session::WindowView::default();
        local.id = local_workspace;
        local.open = true;
        WorkspaceStore::install_for_test(cx, crate::core::session::WindowViews { views: vec![remote, local], active: Some(workspace) });
        launch_attempt(cx, host, target.clone());
    });
    wait_for_link(cx, host);
    let first = attachment(&observer, remote_id);
    assert_eq!(first.hostname, remote_connect::client_hostname());

    // Lose the actual transport while this window is away. A different real
    // control client acquires the workspace before its reconnect can run.
    fixture.restart_router();
    cx.update(|cx| {
        remote_connect::HostLinks::remove(cx, host);
        RemoteLinks::mark(cx, host, |link| {
            link.state = LinkState::Reconnecting;
            link.attach_sent.clear();
        });
    });
    let other = fixture.connect_control_as(&target.connection_key(), "isolated-active-client", "isolated-other-desktop");
    owned.0 = other.clone();
    assert!(matches!(other.client().call(ControlRequest::WorkspaceAttach { id: remote_id.to_string() }).unwrap(), ReplyOk::Attached { .. }));
    assert_eq!(attachment(&other, remote_id).token, "isolated-active-client");
    cx.update(|cx| launch_attempt(cx, host, target.clone()));
    wait_for_link(cx, host);
    assert_eq!(attachment(&other, remote_id).token, "isolated-active-client", "automatic reconnect stole the active client's workspace");
    cx.update(|cx| {
        assert_eq!(RemoteLinks::status_of(cx, workspace), Some(RemoteStatus::Preempted { by: "isolated-other-desktop".into() }));
        pump_attachments(cx, host);
    });
    cx.background_executor.run_until_parked();
    cx.run_until_parked();
    assert_eq!(attachment(&other, remote_id).token, "isolated-active-client", "the next supervisor pass stole the yielded workspace");
    cx.update(|cx| {
        RemoteLinks::retry_now(cx, workspace);
        pump_attachments(cx, host);
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if attachment(&other, remote_id).token != "isolated-active-client" {
            break;
        }
        assert!(Instant::now() < deadline, "explicit Take Back failed to acquire the workspace");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(attachment(&other, remote_id).hostname, remote_connect::client_hostname());
    assert_eq!(other_window.read_with(cx, |app, _| app.workspace), local_workspace, "unrelated rooted window remains valid");
}
