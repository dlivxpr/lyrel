// Real pinned SSH workspace ownership through the rooted workbench supervisor.
use super::*;
use gpui_kit::{TestAppContext, VisualContext as _};

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
        assert!(
            Instant::now() < deadline,
            "real native SSH reconnect did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn attachment(host: &RemoteHost, workspace: WorkspaceId) -> lyrel_core::core::machine::Attachment {
    match host
        .client()
        .call(ControlRequest::WorkspaceTree { workspace })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree.attachment.clone().expect("workspace holder"),
        other => panic!("expected real workspace tree, got {other:?}"),
    }
}

struct OwnedWorkspace(Arc<RemoteHost>, WorkspaceId);
impl Drop for OwnedWorkspace {
    fn drop(&mut self) {
        let _ = self
            .0
            .client()
            .call(ControlRequest::WorkspaceRemove { workspace: self.1 });
    }
}

#[gpui_kit::test]
#[ignore = "requires explicit isolated pinned native SSH/SFTP and same-commit server; run alone"]
fn native_ssh_workspace_reconnect_yields_to_active_client(cx: &mut TestAppContext) {
    let binary = PathBuf::from(
        std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit local server"),
    );
    let mut fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    lyrel_core::core::config::set_config_dir(fixture.local_config_dir());
    let observer = fixture.connect_control();
    let remote_id = WorkspaceId::new();
    assert!(matches!(
        observer
            .client()
            .call(ControlRequest::WorkspaceCreate {
                name: Some("isolated reconnect ownership".into()),
                workspace: Some(remote_id),
            })
            .unwrap(),
        ReplyOk::WorkspaceTree(_)
    ));
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
        crate::ui::windows::WindowRegistry::register(
            cx,
            workspace,
            vcx.window_handle(),
            app.downgrade(),
        );
        crate::ui::windows::WindowRegistry::register(
            cx,
            local_workspace,
            other_vcx.window_handle(),
            other_window.downgrade(),
        );
        let mut remote = lyrel_ui_base::session::WindowView::on_remote(RemoteRef::new(
            target.clone(),
            remote_id,
        ));
        remote.id = workspace;
        remote.open = true;
        let local = lyrel_ui_base::session::WindowView {
            id: local_workspace,
            open: true,
            ..Default::default()
        };
        WorkspaceStore::install_for_test(
            cx,
            lyrel_ui_base::session::WindowViews {
                views: vec![remote, local],
                active: Some(workspace),
            },
        );
        launch_attempt(cx, host, target.clone());
    });
    wait_for_link(cx, host);
    let first = attachment(&observer, remote_id);
    assert_eq!(first.hostname, remote_connect::client_hostname());

    // Lose the actual transport while this window is away. A different real
    // control client acquires the workspace before its reconnect can run.
    fixture.restart_router();
    cx.update(|cx| {
        desktop_lookups::HostLinks::remove(cx, host);
        RemoteLinks::mark(cx, host, |link| {
            link.state = LinkState::Reconnecting;
            link.attach_sent.clear();
        });
    });
    let other = fixture.connect_control_as(
        &target.connection_key(),
        "isolated-active-client",
        "isolated-other-desktop",
    );
    owned.0 = other.clone();
    assert!(matches!(
        other
            .client()
            .call(ControlRequest::WorkspaceAttach {
                id: remote_id.to_string()
            })
            .unwrap(),
        ReplyOk::Attached { .. }
    ));
    assert_eq!(
        attachment(&other, remote_id).hostname,
        "isolated-other-desktop"
    );
    cx.update(|cx| launch_attempt(cx, host, target.clone()));
    wait_for_link(cx, host);
    assert_eq!(
        attachment(&other, remote_id).hostname,
        "isolated-other-desktop",
        "automatic reconnect stole the active client's workspace"
    );
    cx.update(|cx| {
        assert_eq!(
            RemoteLinks::status_of(cx, workspace),
            Some(RemoteStatus::Preempted {
                by: "isolated-other-desktop".into()
            })
        );
        pump_attachments(cx, host);
    });
    cx.background_executor.run_until_parked();
    cx.run_until_parked();
    assert_eq!(
        attachment(&other, remote_id).hostname,
        "isolated-other-desktop",
        "the next supervisor pass stole the yielded workspace"
    );
    cx.update(|cx| {
        RemoteLinks::retry_now(cx, workspace);
        pump_attachments(cx, host);
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if attachment(&other, remote_id).hostname != "isolated-other-desktop" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "explicit Take Back failed to acquire the workspace"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        attachment(&other, remote_id).hostname,
        remote_connect::client_hostname()
    );
    assert_eq!(
        other_window.read_with(cx, |app, _| app.workspace),
        local_workspace,
        "unrelated rooted window remains valid"
    );
}

#[gpui_kit::test]
#[ignore = "requires isolated pinned SSH, matching servers and LYREL_TEST_SSH_PI pointing to official Pi 1.4.0 on the fixture host"]
fn native_ssh_history_resumes_in_its_own_directory_and_preserves_the_busy_pane(
    cx: &mut TestAppContext,
) {
    use crate::ui::agent_launch::ResumeSessionRequest;
    use lyrel_core::core::cli_agent::CLIAgent;
    use lyrel_core::host::Host as _;
    use lyrel_core::host::session_operations::SessionScope;
    use rio_vt::crosswords::grid::Dimensions as _;
    use rio_vt::crosswords::pos::{Column, Line};

    let binary = PathBuf::from(
        std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit local server"),
    );
    let pi = std::env::var("LYREL_TEST_SSH_PI").expect("isolated Linux Pi executable");
    assert!(pi.starts_with('/') && !pi.contains(['\'', '\n', '\r']));
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    lyrel_core::core::config::set_config_dir(fixture.local_config_dir());
    let (app, mut view) = crate::ui::app::test_window::harness(cx);
    let observer = fixture.connect_control();
    let source = fixture.remote_path("project");
    let elsewhere = fixture.remote_path("elsewhere");
    let store = fixture.remote_path("sessions");
    let agent_home = fixture.remote_path("pi-agent");
    for directory in [&source, &elsewhere, &store, &agent_home] {
        observer.create_dir(directory, true).unwrap();
    }
    let file = observer.join(&store, "resume.jsonl");
    let native_id = "63630000-0000-4000-8000-000000000365";
    let title = "Managed Pi history proof";
    let timestamp = "2026-11-21T00:00:00Z";
    let header = serde_json::json!({"type":"session", "version":3, "id":native_id, "timestamp":timestamp, "cwd":source});
    let message = serde_json::json!({"type":"message", "id":"a1000001", "parentId":null, "timestamp":timestamp, "message":{"role":"user", "content":title, "timestamp":1795219200000_u64}});
    observer
        .write_file(&file, format!("{header}\n{message}\n").as_bytes())
        .unwrap();
    observer.write_file(&observer.join(&agent_home, "settings.json"), br#"{"quietStartup":true,"enableInstallTelemetry":false,"enableAnalytics":false,"cacheWarming":"off","compaction":{"enabled":false}}"#).unwrap();

    let remote = WorkspaceId::new();
    assert!(matches!(
        observer
            .client()
            .call(ControlRequest::WorkspaceCreate {
                name: Some("isolated history resume".into()),
                workspace: Some(remote),
            })
            .unwrap(),
        ReplyOk::WorkspaceTree(_)
    ));
    let _owned = OwnedWorkspace(observer.clone(), remote);
    let spec = fixture.spec();
    let mut profile = crate::core::ssh_profile::SshProfile::new("isolated history resume");
    profile.host = spec.host;
    profile.port = spec.port;
    profile.user = spec.user;
    profile.identity_files = spec.identity_files;
    profile.auth = crate::core::ssh_profile::AuthMode::PublicKey;
    profile.verify_host_keys = Some(true);
    let target = RemoteTarget::Profile { id: profile.id };
    let host = target.host_id();
    let workspace = app.read_with(cx, |app, _| app.workspace);
    let handle = view.window_handle();
    cx.update(|cx| {
        let config = cx.global_mut::<Config>();
        config.ssh_profiles.push(profile);
        config.agent_launch.insert(
            "pi".into(),
            format!("'{pi}' --offline --no-extensions --no-skills --no-mcp --no-context-files"),
        );
        crate::ui::windows::WindowRegistry::register(cx, workspace, handle, app.downgrade());
        let mut view =
            lyrel_ui_base::session::WindowView::on_remote(RemoteRef::new(target.clone(), remote));
        view.id = workspace;
        view.open = true;
        WorkspaceStore::install_for_test(
            cx,
            lyrel_ui_base::session::WindowViews {
                views: vec![view],
                active: Some(workspace),
            },
        );
        launch_attempt(cx, host, target);
    });
    wait_for_link(cx, host);
    assert_eq!(
        attachment(&observer, remote).hostname,
        remote_connect::client_hostname()
    );

    view.simulate_resize(gpui_kit::size(gpui_kit::px(1900.), gpui_kit::px(900.)));
    app.update_in(&mut view, |app, window, cx| {
        app.new_tab_slot(Some(elsewhere.clone()), None, window, cx)
            .expect("remote shell slot");
    });
    let wait = |view: &mut gpui_kit::VisualTestContext,
                label,
                condition: &mut dyn FnMut(&mut gpui_kit::App) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            view.executor().advance_clock(Duration::from_millis(50));
            view.run_until_parked();
            if view.update(|_, cx| condition(cx)) {
                break;
            }
            assert!(Instant::now() < deadline, "{label}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let active_terminals = |cx: &gpui_kit::App| {
        let tabs = &app.read(cx).tabs;
        tabs[tabs.active_index()].pane.terminals()
    };
    let mut panes = Vec::new();
    wait(
        &mut view,
        "managed shell did not report its prompt and cwd",
        &mut |cx| {
            panes = active_terminals(cx);
            panes.first().is_some_and(|pane| {
                let pane = pane.read(cx);
                pane.host_id() == host
                    && pane.host_cwd().as_deref() == Some(elsewhere.as_path())
                    && pane.terminal.at_prompt()
            })
        },
    );
    let original = panes.remove(0);
    fixture.track_pane(original.read_with(cx, |pane, _| pane.pane_id));
    original.update(&mut view, |pane, _| pane.run_command_line("sleep 120"));
    wait(
        &mut view,
        "the original pane did not start sleep",
        &mut |cx| {
            let pane = original.read(cx);
            !pane.terminal.at_prompt() && pane.terminal.running_command().contains("sleep 120")
        },
    );

    let mut scope = SessionScope::default();
    scope.environment.insert(
        "PI_CODING_AGENT_DIR".into(),
        agent_home.to_string_lossy().into_owned(),
    );
    scope.session_dir = Some(store);
    scope.session_file = Some(file);
    app.update_in(&mut view, |app, window, cx| {
        app.resume_session(
            ResumeSessionRequest {
                agent: CLIAgent::Pi,
                session_id: native_id.into(),
                cwd: Some(source.clone()),
                host_id: host,
                scope: Some(scope),
                fork: false,
            },
            window,
            cx,
        );
    });
    let mut restored_id = None;
    wait(
        &mut view,
        "the right split did not display the saved Pi message",
        &mut |cx| {
            let panes = active_terminals(cx);
            let [first, second] = panes.as_slice() else {
                return false;
            };
            assert_eq!(
                first.entity_id(),
                original.entity_id(),
                "the original pane still occupies the left side"
            );
            let pane = second.read(cx);
            if restored_id.is_none() && pane.host_id() == host {
                fixture.track_pane(pane.pane_id);
                restored_id = Some(pane.pane_id);
            }
            if pane.host_id() != host || pane.host_cwd().as_deref() != Some(source.as_path()) {
                return false;
            }
            let terminal = pane.terminal.term.lock();
            let grid = &terminal.grid;
            (0..grid.screen_lines() as i32).any(|line| {
                let line: String = (0..grid.columns())
                    .map(|col| grid[Line(line)][Column(col)].c())
                    .collect();
                line.contains(title)
            })
        },
    );
    assert!(
        original.read_with(cx, |pane, _| {
            !pane.terminal.at_prompt()
                && !pane.terminal.exited
                && pane.terminal.running_command().contains("sleep 120")
        }),
        "the original pane's program remains running"
    );
}
