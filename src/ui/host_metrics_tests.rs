//! Consumer regressions through a real application window and real Host server.
use super::*;
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};
use lyrel_core::daemon::control::{ControlClient, ControlHello};

use crate::core::actions::{
    EditorNewFile, RestorePanePip, SelectNextTab, ToggleDocumentFill, TogglePanePip,
};
use crate::core::session::{
    RemoteRef, RemoteTarget, WindowView, WindowViews, WorkspaceId, WorkspaceStore,
};
use crate::ui::app::test_window::harness_with_tabs;

pub(crate) struct HostServer {
    pub(crate) client: Arc<ControlClient>,
    thread: Option<JoinHandle<()>>,
}

impl HostServer {
    pub(crate) fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let thread = std::thread::spawn(move || {
            lyrel_core::host::server::serve(server, lyrel_core::host::local::LocalHost::new())
                .unwrap();
        });
        let client = Arc::new(
            ControlClient::over_tcp(
                stream,
                &ControlHello::host_rpc("metrics-window-test", "test-window"),
                Box::new(|_| {}),
            )
            .unwrap(),
        );
        Self {
            client,
            thread: Some(thread),
        }
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.client.close();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

fn has_sample(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, _| {
        window
            .try_find("status-cpu")
            .and_then(|cpu| cpu.label().map(str::to_owned))
            .is_some_and(|label| label.ends_with('%'))
    })
}

pub(crate) fn wait_sample(vcx: &mut VisualTestContext, wanted: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(vcx);
        if has_sample(vcx) == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "host sample availability did not become {wanted}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn assert_metrics_status(vcx: &mut VisualTestContext, key: crate::ui::i18n::L10nKey) {
    vcx.update(|window, _| window.refresh());
    settle(vcx);
    vcx.update(|window, _| {
        assert_eq!(
            window.find("status-host-metrics").label(),
            Some(crate::ui::i18n::t(key)),
        );
    });
    assert!(
        !has_sample(vcx),
        "a pending link displayed an old host sample"
    );
}

#[gpui_kit::test]
fn host_samples_follow_workspace_and_recover_after_link_loss(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _pane_streams) = harness_with_tabs(cx, 2);
    vcx.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(
            cx,
            app.read(cx).workspace,
            window.window_handle(),
            app.downgrade(),
        );
        LocalLink::install_client_for_test(cx, server.client.clone());
    });
    wait_sample(&mut vcx, true);
    let client = server.client.clone();
    let hostname = std::thread::spawn(move || client.host_metrics().unwrap().hostname().to_owned())
        .join()
        .unwrap();
    vcx.dispatch_action(SelectNextTab);
    settle(&mut vcx);
    assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
    vcx.update(|window, _| {
        assert!(
            window
                .find("status-host-metrics")
                .label()
                .unwrap()
                .contains(&hostname)
        );
    });
    vcx.dispatch_action(TogglePanePip);
    settle(&mut vcx);
    assert!(app.read_with(&vcx, |app, cx| app.tabs[app.active].pip_slot(cx).is_some()));
    vcx.update(|window, _| {
        assert!(
            window
                .find("status-host-metrics")
                .label()
                .unwrap()
                .contains(&hostname)
        );
    });
    assert!(has_sample(&mut vcx));
    vcx.dispatch_action(RestorePanePip);
    settle(&mut vcx);
    assert!(app.read_with(&vcx, |app, cx| app.tabs[app.active].pip_slot(cx).is_none()));
    for (width, font) in [(1440., 16.), (720., 20.)] {
        vcx.simulate_resize(size(px(width), px(720.)));
        app.update_in(&mut vcx, |_, _, cx| {
            cx.global_mut::<crate::core::config::Config>().ui_font_size = font;
            cx.notify();
        });
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert!(window.find("status-cpu").visible());
            assert!(window.find("status-memory").visible());
            let bar = window.find("workbench-status").bounds();
            let metrics = window.find("status-host-metrics").bounds();
            assert!(metrics.right() <= bar.right());
            assert!(metrics.bottom() <= bar.bottom());
        });
    }

    // A document workflow replaces, rather than duplicates, the terminal widgets.
    vcx.dispatch_action(EditorNewFile);
    vcx.dispatch_action(ToggleDocumentFill);
    settle(&mut vcx);
    vcx.update(|window, _| assert!(window.try_find("status-host-metrics").is_none()));
    vcx.update(|window, cx| window.click("editor-panel-close", cx));
    wait_sample(&mut vcx, true);

    let local = app.read_with(&vcx, |app, _| app.workspace);
    let remote = WindowView::on_remote(RemoteRef::new(
        RemoteTarget::Alias {
            alias: "metrics-no-connection".into(),
        },
        WorkspaceId::new(),
    ));
    let remote_id = remote.id;
    app.update_in(&mut vcx, |app, window, cx| {
        let mut views = WorkspaceStore::all(cx).clone();
        views.views.push(remote);
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: views.views,
                active: None,
            },
        );
        app.switch_workspace(Some(remote_id), window, cx);
    });
    settle(&mut vcx);
    assert!(
        !has_sample(&mut vcx),
        "remote workspace relabeled the local host sample"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.switch_workspace(Some(local), window, cx)
    });
    wait_sample(&mut vcx, true);

    // Losing this daemon clears numbers; no local-process or cached fallback.
    server.client.close();
    wait_sample(&mut vcx, false);
    let reconnected = HostServer::new();
    vcx.update(|_, cx| LocalLink::install_client_for_test(cx, reconnected.client.clone()));
    wait_sample(&mut vcx, true);
}

struct RemoteServer {
    host: Arc<lyrel_core::host::remote::RemoteHost>,
    thread: Option<JoinHandle<()>>,
}

impl RemoteServer {
    fn new(target: &RemoteTarget) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let thread = std::thread::spawn(move || {
            lyrel_core::host::server::serve(server, lyrel_core::host::local::LocalHost::new())
                .unwrap();
        });
        let host = lyrel_core::host::remote::RemoteHost::over_tcp(
            stream,
            &target.connection_key(),
            &ControlHello::host_rpc("remote-metrics-window", "consumer"),
        )
        .unwrap();
        Self {
            host,
            thread: Some(thread),
        }
    }

    fn install(&self, vcx: &mut VisualTestContext) {
        vcx.update(|_, cx| {
            crate::ui::remote_connect::HostLinks::insert(cx, self.host.clone(), self.host.home())
        });
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.host.client().close();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[gpui_kit::test]
fn remote_workspace_metrics_never_relabel_another_hosts_sample(cx: &mut TestAppContext) {
    let target_a = RemoteTarget::Alias {
        alias: "metrics-route-a".into(),
    };
    let target_b = RemoteTarget::Alias {
        alias: "metrics-route-b".into(),
    };
    let server_a = RemoteServer::new(&target_a);
    let (app, mut vcx, _pane_streams) = harness_with_tabs(cx, 2);
    vcx.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(
            cx,
            app.read(cx).workspace,
            window.window_handle(),
            app.downgrade(),
        );
        cx.set_global(LocalLink::default());
    });
    let local = app.read_with(&vcx, |app, _| app.workspace);
    let workspace_a = WindowView::on_remote(RemoteRef::new(target_a.clone(), WorkspaceId::new()));
    let workspace_b = WindowView::on_remote(RemoteRef::new(target_b.clone(), WorkspaceId::new()));
    let a = workspace_a.id;
    let b = workspace_b.id;
    app.update_in(&mut vcx, |_, _, cx| {
        let mut views = WorkspaceStore::all(cx).clone();
        views.views.extend([workspace_a, workspace_b]);
        WorkspaceStore::install_for_test(cx, views);
    });
    server_a.install(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.switch_workspace(Some(a), window, cx)
    });
    wait_sample(&mut vcx, true);
    // No local client exists: these numbers can only have arrived over A's Host.
    assert!(vcx.update(|_, cx| LocalLink::client(cx).is_none()));

    app.update_in(&mut vcx, |app, window, cx| {
        app.switch_workspace(Some(b), window, cx)
    });
    settle(&mut vcx);
    assert!(!has_sample(&mut vcx), "B briefly displayed A's old numbers");
    let server_b = RemoteServer::new(&target_b);
    server_b.install(&mut vcx);
    wait_sample(&mut vcx, true);
    server_a.host.client().close();
    settle(&mut vcx);
    assert!(
        has_sample(&mut vcx),
        "A's disconnect cleared B's measurement"
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.switch_workspace(Some(a), window, cx)
    });
    settle(&mut vcx);
    assert!(
        !has_sample(&mut vcx),
        "a disconnected A retained B's sample"
    );
    let reconnected_a = RemoteServer::new(&target_a);
    reconnected_a.install(&mut vcx);
    wait_sample(&mut vcx, true);

    app.update_in(&mut vcx, |app, window, cx| {
        app.switch_workspace(Some(local), window, cx)
    });
    settle(&mut vcx);
    assert!(
        !has_sample(&mut vcx),
        "local workspace reused A despite no local daemon connection"
    );
    let local_server = HostServer::new();
    vcx.update(|_, cx| LocalLink::install_client_for_test(cx, local_server.client.clone()));
    wait_sample(&mut vcx, true);
}

#[gpui_kit::test]
fn metrics_completion_uses_daemon_identity_without_replaying_old_link_errors(
    cx: &mut TestAppContext,
) {
    let local_a = HostServer::new();
    let local_b = HostServer::new();
    let target = RemoteTarget::Alias {
        alias: "metrics-completion-route".into(),
    };
    let remote_a = RemoteServer::new(&target);
    let remote_b = RemoteServer::new(&target);
    let cases = [
        (
            HostId::LOCAL,
            MetricsSource::Local(local_a.client.clone()),
            MetricsSource::Local(local_b.client.clone()),
        ),
        (
            remote_a.host.id(),
            MetricsSource::Remote(remote_a.host.clone()),
            MetricsSource::Remote(remote_b.host.clone()),
        ),
    ];
    for (host, source, replacement) in cases {
        let install = |route: &MetricsSource, cx: &mut App| match route {
            MetricsSource::Local(client) => LocalLink::install_client_for_test(cx, client.clone()),
            MetricsSource::Remote(host) => HostLinks::insert(cx, host.clone(), host.home()),
        };
        cx.update(|cx| install(&source, cx));
        let query = source.clone();
        let snapshot = std::thread::spawn(move || query.sample().unwrap())
            .join()
            .unwrap();
        // Complete the old request only after its control route was replaced.
        // Both real connections reach the same server process, not the same Arc.
        assert_eq!(source.instance(), replacement.instance());
        cx.update(|cx| install(&replacement, cx));
        match &source {
            MetricsSource::Local(client) => client.close(),
            MetricsSource::Remote(host) => host.client().close(),
        }
        cx.update(|cx| {
            assert!(matches!(
                source.response_state(host, Ok(snapshot.clone()), cx),
                State::Ready(accepted) if accepted == snapshot
            ));
            for kind in [std::io::ErrorKind::BrokenPipe, std::io::ErrorKind::Other] {
                assert!(
                    matches!(
                        source.response_state(host, Err(std::io::Error::from(kind)), cx),
                        State::Waiting
                    ),
                    "an old connection's error contaminated the healthy replacement"
                );
            }
            assert!(matches!(
                replacement.response_state(host, Err(std::io::Error::other("sampling denied")), cx),
                State::Failed(reason) if reason == "sampling denied"
            ));
            assert!(matches!(
                replacement.response_state(host, Err(std::io::ErrorKind::Unsupported.into()), cx),
                State::Unsupported
            ));
            let mut stale = serde_json::to_value(&snapshot).unwrap();
            stale["instance"] = "previous-daemon-instance".into();
            assert!(
                matches!(
                    source.response_state(host, Ok(serde_json::from_value(stale).unwrap()), cx),
                    State::Waiting
                ),
                "a previous daemon's completion was accepted by its replacement"
            );
        });
    }
}
