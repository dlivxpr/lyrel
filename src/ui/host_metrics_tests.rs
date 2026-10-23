//! Consumer regressions through a real application window and real Host server.
use super::*;
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};
use lyrel_core::daemon::control::ControlHello;

use crate::core::actions::{EditorNewFile, SelectNextTab, ToggleDocumentFill};
use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WindowViews};
use crate::ui::app::test_window::harness_with_tabs;

struct HostServer {
    client: Arc<ControlClient>,
    thread: Option<JoinHandle<()>>,
}

impl HostServer {
    fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let thread = std::thread::spawn(move || {
            lyrel_core::host::server::serve(server, lyrel_core::host::local::LocalHost::new()).unwrap();
        });
        let client = Arc::new(ControlClient::over_tcp(stream,
            &ControlHello::host_rpc("metrics-window-test", "test-window"), Box::new(|_| {})).unwrap());
        Self { client, thread: Some(thread) }
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.client.close();
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
    }
}

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

fn has_sample(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, _| {
        window.try_find("status-cpu").and_then(|cpu| cpu.label().map(str::to_owned))
            .is_some_and(|label| label.ends_with('%'))
    })
}

fn wait_sample(vcx: &mut VisualTestContext, wanted: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(vcx);
        if has_sample(vcx) == wanted { return; }
        assert!(Instant::now() < deadline, "host sample availability did not become {wanted}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui_kit::test]
fn host_samples_follow_workspace_and_recover_after_link_loss(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _pane_streams) = harness_with_tabs(cx, 2);
    vcx.update(|_, cx| cx.default_global::<LocalLink>().client = Some(server.client.clone()));
    wait_sample(&mut vcx, true);
    let client = server.client.clone();
    let hostname = std::thread::spawn(move || client.host_metrics().unwrap().hostname().to_owned())
        .join().unwrap();
    vcx.dispatch_action(SelectNextTab);
    settle(&mut vcx);
    assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
    vcx.update(|window, _| {
        assert!(window.find("status-host-metrics").label().unwrap().contains(&hostname));
    });
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
    let remote = WindowView::on_remote(RemoteRef::new(RemoteTarget::Alias {
        alias: "metrics-no-connection".into(),
    }, WorkspaceId::new()));
    let remote_id = remote.id;
    app.update_in(&mut vcx, |app, window, cx| {
        let mut views = WorkspaceStore::all(cx).clone();
        views.views.push(remote);
        WorkspaceStore::install_for_test(cx, WindowViews { views: views.views, active: None });
        app.switch_workspace(Some(remote_id), window, cx);
    });
    settle(&mut vcx);
    assert!(!has_sample(&mut vcx), "remote workspace relabeled the local host sample");
    app.update_in(&mut vcx, |app, window, cx| app.switch_workspace(Some(local), window, cx));
    wait_sample(&mut vcx, true);

    // Losing this daemon clears numbers; no local-process or cached fallback.
    server.client.close();
    wait_sample(&mut vcx, false);
    let reconnected = HostServer::new();
    vcx.update(|_, cx| cx.default_global::<LocalLink>().client = Some(reconnected.client.clone()));
    wait_sample(&mut vcx, true);
}
