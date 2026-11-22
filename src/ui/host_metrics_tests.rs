//! Consumer regressions through a real application window and real Host server.
use super::*;
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::daemon::control::{ControlClient, ControlHello};

use crate::ui::app::test_window::harness_with_tabs;
use lyrel_ui_base::actions::{
    EditorNewFile, RestorePanePip, SelectNextTab, ToggleDocumentFill, TogglePanePip,
};
use lyrel_ui_base::i18n;
use lyrel_ui_base::session::{
    RemoteRef, RemoteTarget, WindowView, WindowViews, WorkspaceId, WorkspaceStore,
};

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
            lyrel_core::host::server::serve(server, lyrel_core::host::local::new()).unwrap();
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

/// One reading stamped exactly where the test wants it on the timeline.
fn sample_at(at_millis: u64, cpu: f32, used_gib: u64, total_gib: u64) -> HostSample {
    HostSample::new(
        cpu,
        used_gib * 1_073_741_824,
        total_gib * 1_073_741_824,
        at_millis,
    )
}

#[test]
fn history_aggregates_peaks_per_slot_and_leaves_unsampled_slots_blank() {
    let mut history = SampleHistory::default();
    // A 15-minute window divides into 15-second slots: two readings in one
    // slot keep the peak, and a slot nobody reached stays blank.
    history.push(sample_at(0, 5., 8, 16), "instance-a");
    history.push(sample_at(0, 95., 8, 16), "instance-a");
    let anchor = history.latest().map(|arrival| arrival.arrived_millis).unwrap();
    let buckets = history.buckets(MetricsRange::QuarterHour, anchor);
    assert_eq!(buckets.len(), BUCKET_COUNT);
    // The readings are the freshest thing in the window, so they occupy the
    // last slot — and two readings in one slot keep the peak.
    let peak = buckets[BUCKET_COUNT - 1].expect("the last slot holds the readings");
    assert_eq!(peak.cpu, 95., "a long view keeps the peak, not the average");
    assert!(
        buckets[..BUCKET_COUNT - 1].iter().all(Option::is_none),
        "unsampled time must stay blank instead of reading as a quiet zero"
    );

    // Memory rides in the same slots, percentages against the total.
    let memory = buckets[BUCKET_COUNT - 1]
        .map(|bucket| bucket.memory)
        .unwrap_or_default();
    assert_eq!(memory, 50.);
}

#[test]
fn history_drops_readings_older_than_the_widest_window() {
    let mut history = SampleHistory::default();
    // Arrival stamps advance with the GUI clock, not the wire's clock.
    history.push(sample_at(0, 10., 8, 16), "instance-a");
    history.push(sample_at(0, 20., 8, 16), "instance-a");
    history.push(sample_at(0, 30., 8, 16), "instance-a");
    assert!(history.arrivals.len() >= 2, "the span limit is inclusive");
    assert_eq!(
        history.arrivals.front().map(|a| a.arrived_millis)
            <= history.arrivals.back().map(|a| a.arrived_millis),
        true,
        "readings are kept in arrival order for the span trim"
    );
    // A zero total is not a memory reading and never enters the history.
    history.push(HostSample::new(1., 1, 0, 2_000), "instance-a");
    assert_eq!(history.arrivals.len(), 3);
}

#[test]
fn a_new_daemon_instance_drops_the_predecessors_samples() {
    let mut history = SampleHistory::default();
    history.push(sample_at(0, 10., 8, 16), "old-daemon");
    assert_eq!(history.arrivals.len(), 1);
    history.push(sample_at(0, 20., 8, 16), "new-daemon");
    assert_eq!(
        history.arrivals.len(),
        1,
        "a peer instance change must not present its predecessor's samples"
    );
    assert_eq!(
        history.instance.as_deref(),
        Some("new-daemon"),
        "the history is restamped by the answering daemon's identity"
    );
}

fn has_titlebar_sample(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, _| {
        window
            .try_find("titlebar-metric-cpu-value")
            .and_then(|cpu| cpu.label().map(str::to_owned))
            .is_some_and(|label| label.ends_with('%'))
    })
}

pub(crate) fn wait_sample(vcx: &mut VisualTestContext, wanted: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(vcx);
        if has_titlebar_sample(vcx) == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "host sample availability did not become {wanted}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The title-bar metric's state word, asserted while no stale sample hides
/// it. Shared with the LocalLink lifecycle tests.
pub(crate) fn assert_metrics_status(vcx: &mut VisualTestContext, key: i18n::L10nKey) {
    vcx.update(|window, _| window.refresh());
    settle(vcx);
    vcx.update(|window, _| {
        assert_eq!(
            window.find("titlebar-metric-cpu").label(),
            Some(i18n::t(key)),
        );
    });
    assert!(
        !has_titlebar_sample(vcx),
        "a pending link displayed an old host sample"
    );
}

#[gpui_kit::test]
fn titlebar_metrics_track_the_context_host_and_click_opens_monitor(
    cx: &mut TestAppContext,
) {
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
    let hostname = std::thread::spawn(move || {
        server.client.host_metrics().unwrap().hostname().to_owned()
    })
    .join()
    .unwrap();

    // Both metrics render with the host's own reading in their detail.
    vcx.update(|window, _| {
        for id in ["titlebar-metric-cpu", "titlebar-metric-ram"] {
            let metric = window.find(id);
            assert!(metric.visible(), "{id} must render in the chrome");
            assert!(
                metric.label().unwrap().contains(&hostname),
                "{id} must name the context pane's host"
            );
        }
        assert!(window.find("titlebar-metric-cpu-chart").visible());
        assert!(window.find("titlebar-metric-ram-chart").visible());
    });

    // The former bottom-bar indicators are gone.
    vcx.update(|window, _| {
        assert!(window.try_find("status-host-metrics").is_none());
    });

    // One shared sampler answers the title bar, and clicking either metric
    // opens the Activity Monitor on those same samples.
    vcx.update(|window, cx| window.click("titlebar-metric-cpu", cx));
    settle(&mut vcx);
    assert!(app.read_with(&vcx, |app, _| {
        app.right_panel_tab == RightPanelTab::Monitor && app.right_panel_visible
    }));
    vcx.update(|window, _| {
        assert!(window.find("monitor-tile-cpu").visible());
        assert!(window.find("monitor-tile-ram").visible());
        assert!(window.find("monitor-range-selector").visible());
        assert_eq!(
            window.find("monitor-tile-cpu").label(),
            window.find("titlebar-metric-cpu").label(),
            "title bar and monitor must present the same sample"
        );
        assert_eq!(
            window.find("monitor-range-120").label(),
            Some(i18n::t(L10nKey::MonitorRangeTwoMinutes)),
            "the approved windows start at 2 minutes"
        );
    });
    assert_eq!(
        vcx.update(|_, cx| {
            cx.default_global::<MetricsViews>()
                .0
                .values()
                .filter(|view| view.upgrade().is_some())
                .count()
        }),
        1,
        "title bar and monitor share the one live sampler — no competing loops"
    );
}

#[gpui_kit::test]
fn monitor_history_survives_being_offscreen(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _pane_streams) = harness_with_tabs(cx, 1);
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

    // Open the monitor, let history accumulate, then put the metrics fully
    // offscreen: the panel's tab closed and the document surface replacing
    // terminal chrome (and with it the title-bar strip).
    vcx.update(|window, cx| window.click("titlebar-metric-ram", cx));
    settle(&mut vcx);
    for _ in 0..3 {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
    }
    let before = vcx.update(|_, cx| {
        cx.default_global::<MetricsHistories>()
            .0
            .get(&HostId::LOCAL)
            .map_or(0, |history| history.arrivals.len())
    });
    assert!(before > 0, "sampling must accumulate before the check");

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_tab(RightPanelTab::Monitor, window, cx);
    });
    vcx.dispatch_action(EditorNewFile);
    vcx.dispatch_action(ToggleDocumentFill);
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(window.try_find("titlebar-metrics").is_none());
        assert!(window.try_find("monitor-tile-cpu").is_none());
    });

    // Demand-driven polling: with nobody showing the host, the sampler view
    // is released and its loop stops — but the run-scoped history survives.
    for _ in 0..3 {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
    }
    let frozen = vcx.update(|_, cx| {
        cx.default_global::<MetricsHistories>()
            .0
            .get(&HostId::LOCAL)
            .map_or(0, |history| history.arrivals.len())
    });
    assert_eq!(
        frozen, before,
        "a temporarily offscreen view stops polling but must not destroy history"
    );

    // Back on screen: the history is still there and sampling resumes,
    // leaving an honest local-time gap where nothing was collected.
    vcx.update(|window, cx| window.click("editor-panel-close", cx));
    settle(&mut vcx);
    vcx.update(|window, cx| window.click("titlebar-metric-cpu", cx));
    settle(&mut vcx);
    for _ in 0..2 {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
    }
    let resumed = vcx.update(|_, cx| {
        cx.default_global::<MetricsHistories>()
            .0
            .get(&HostId::LOCAL)
            .map_or(0, |history| history.arrivals.len())
    });
    assert!(
        resumed > frozen,
        "coming back on screen resumes sampling on the retained history"
    );
}

#[gpui_kit::test]
fn narrow_windows_drop_charts_first_then_the_whole_strip(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _pane_streams) = harness_with_tabs(cx, 1);
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

    for (width, charts, strip) in [(1440., true, true), (720., false, true), (500., false, false)] {
        vcx.simulate_resize(size(px(width), px(720.)));
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert_eq!(
                window.try_find("titlebar-metrics").is_some(),
                strip,
                "width {width}: strip presence"
            );
            if strip {
                assert_eq!(
                    window.try_find("titlebar-metric-cpu-chart").is_some(),
                    charts,
                    "width {width}: charts are what narrow windows drop first"
                );
                assert!(
                    window.find("titlebar-metric-cpu-value").visible(),
                    "width {width}: readings survive the charts"
                );
            }
            // The chrome around the strip never yields: native-adjacent
            // controls and the drag area stay put at every width.
            for id in [
                "titlebar-split-right",
                "titlebar-split-down",
                "titlebar-right-panel",
            ] {
                assert!(window.find(id).visible(), "width {width}: {id}");
            }
        });
    }
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
    let hostname = std::thread::spawn(move || {
        client.host_metrics().unwrap().hostname().to_owned()
    })
    .join()
    .unwrap();
    vcx.dispatch_action(SelectNextTab);
    settle(&mut vcx);
    assert_eq!(app.read_with(&vcx, |app, _| app.tabs.active_index()), 1);
    vcx.update(|window, _| {
        assert!(
            window
                .find("titlebar-metric-cpu")
                .label()
                .unwrap()
                .contains(&hostname)
        );
    });
    vcx.dispatch_action(TogglePanePip);
    settle(&mut vcx);
    assert!(app.read_with(&vcx, |app, cx| {
        app.tabs[app.tabs.active_index()].pip_slot(cx).is_some()
    }));
    vcx.update(|window, _| {
        assert!(
            window
                .find("titlebar-metric-cpu")
                .label()
                .unwrap()
                .contains(&hostname)
        );
    });
    assert!(has_titlebar_sample(&mut vcx));
    vcx.dispatch_action(RestorePanePip);
    settle(&mut vcx);
    assert!(app.read_with(&vcx, |app, cx| {
        app.tabs[app.tabs.active_index()].pip_slot(cx).is_none()
    }));
    for (width, font) in [(1440., 16.), (720., 20.)] {
        vcx.simulate_resize(size(px(width), px(720.)));
        app.update_in(&mut vcx, |_, _, cx| {
            cx.global_mut::<lyrel_ui_base::config::Config>()
                .ui_font_size = font;
            cx.notify();
        });
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert!(window.find("titlebar-metric-cpu").visible());
            assert!(window.find("titlebar-metric-ram").visible());
            let metrics = window.find("titlebar-metrics").bounds();
            assert!(metrics.right() <= window.viewport_size().width);
            assert!(metrics.top() >= px(0.));
        });
    }

    // A document workflow replaces, rather than duplicates, the terminal widgets.
    vcx.dispatch_action(EditorNewFile);
    vcx.dispatch_action(ToggleDocumentFill);
    settle(&mut vcx);
    vcx.update(|window, _| assert!(window.try_find("titlebar-metrics").is_none()));
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
        !has_titlebar_sample(&mut vcx),
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
            lyrel_core::host::server::serve(server, lyrel_core::host::local::new()).unwrap();
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
            crate::ui::desktop_lookups::HostLinks::insert(cx, self.host.clone(), self.host.home())
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
    assert!(
        !has_titlebar_sample(&mut vcx),
        "B briefly displayed A's old numbers"
    );
    let server_b = RemoteServer::new(&target_b);
    server_b.install(&mut vcx);
    wait_sample(&mut vcx, true);
    server_a.host.client().close();
    settle(&mut vcx);
    assert!(
        has_titlebar_sample(&mut vcx),
        "A's disconnect cleared B's measurement"
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.switch_workspace(Some(a), window, cx)
    });
    settle(&mut vcx);
    assert!(
        !has_titlebar_sample(&mut vcx),
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
        !has_titlebar_sample(&mut vcx),
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
        // 此用例验证连接替换后的结果优先级，不依赖实时 OS 采样的耗时。
        let snapshot: HostMetrics = serde_json::from_value(serde_json::json!({
            "hostname": "metrics-completion-test",
            "instance": source.instance(),
            "state": "waiting",
        }))
        .unwrap();
        // Deliver the old result only after its control route was replaced.
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
