//! Consumer regressions through a real application window and real Host server.
use super::*;
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::daemon::control::{ControlClient, ControlHello};
use lyrel_core::host::metrics::NetworkThroughput;

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
    let anchor = history
        .latest()
        .map(|arrival| arrival.arrived_millis)
        .unwrap();
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

#[test]
fn gpu_readout_shows_a_percent_a_reason_or_an_honest_dash() {
    let snapshot = |sample| HostMetrics::new("gpu-host", "instance", MetricsState::Sample(sample));

    let ready = readout(
        &State::Ready(snapshot(sample_at(0, 10., 8, 16).with_gpu(42.25))),
        false,
    );
    assert_eq!(ready.gpu_text(), "42%");
    assert_eq!(ready.gpu_status(), None);

    // The platform could not supply a reading: the value is the down marker
    // and the slot's state word is the sampler's own reason.
    let unavailable = readout(
        &State::Ready(snapshot(
            sample_at(0, 10., 8, 16).with_gpu_unavailable("no GPU engine performance counters"),
        )),
        false,
    );
    assert_eq!(unavailable.gpu_text(), "—");
    assert_eq!(
        unavailable.gpu_status().map(|status| status.to_string()),
        Some("no GPU engine performance counters".to_owned()),
        "the GPU slot explains the miss with the sampler's reason"
    );

    // A daemon that predates GPU sampling still parses: the slot shows the
    // down marker without inventing a reason.
    let legacy = readout(&State::Ready(snapshot(sample_at(0, 10., 8, 16))), false);
    assert_eq!(legacy.gpu_text(), "—");
    assert_eq!(legacy.gpu_status(), None);

    // Whole-sample pending words are shared with the GPU slot.
    let waiting = readout(&State::Waiting, false);
    assert_eq!(waiting.gpu_text(), "…");
    assert!(waiting.gpu_status().is_some());
}

#[test]
fn history_buckets_keep_gpu_peaks_and_blank_slots_without_gpu() {
    let mut history = SampleHistory::default();
    history.push(sample_at(0, 5., 8, 16).with_gpu(30.), "instance-a");
    history.push(sample_at(0, 50., 8, 16).with_gpu(90.), "instance-a");
    history.push(sample_at(0, 60., 8, 16), "instance-a");
    let anchor = history.latest().unwrap().arrived_millis;
    let buckets = history.buckets(MetricsRange::QuarterHour, anchor);
    assert_eq!(buckets[BUCKET_COUNT - 1].unwrap().gpu, Some(90.));
    let mut history = SampleHistory::default();
    history.push(sample_at(0, 5., 8, 16), "instance-a");
    let anchor = history.latest().unwrap().arrived_millis;
    let buckets = history.buckets(MetricsRange::TwoMinutes, anchor);
    assert_eq!(buckets[BUCKET_COUNT - 1].unwrap().gpu, None);
}

#[test]
fn history_keeps_network_peaks_without_letting_missing_readings_zero_them() {
    let mut history = SampleHistory::default();
    history.push(
        sample_at(0, 5., 8, 16).with_network(NetworkThroughput::new(1_000., 500.)),
        "instance-a",
    );
    // A second reading out-peaks the first.
    history.push(
        sample_at(0, 95., 8, 16).with_network(NetworkThroughput::new(3_000., 1_500.)),
        "instance-a",
    );
    // A CPU/RAM-only sample — a tick whose network reading was missing — must
    // not zero the peaks the real readings set.
    history.push(sample_at(0, 50., 8, 16), "instance-a");
    let anchor = history
        .latest()
        .map(|arrival| arrival.arrived_millis)
        .unwrap();
    let buckets = history.buckets(MetricsRange::QuarterHour, anchor);
    let peak_rx = buckets
        .iter()
        .flatten()
        .map(|bucket| bucket.network_rx)
        .fold(f32::NEG_INFINITY, f32::max);
    let peak_tx = buckets
        .iter()
        .flatten()
        .map(|bucket| bucket.network_tx)
        .fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(
        peak_rx, 3_000.,
        "the network peak survives a tick whose reading was missing"
    );
    assert_eq!(peak_tx, 1_500.);
    // The network-less tick is still a kept sample: only its network field
    // was missing, not the CPU/RAM reading it carried.
    assert_eq!(history.arrivals.len(), 3);
}

#[test]
fn rate_text_uses_link_rate_units_and_trims_the_decimal() {
    assert_eq!(rate_text(0.), "0 B/s");
    assert_eq!(rate_text(512.), "512 B/s");
    assert_eq!(rate_text(240_000.), "240 KB/s");
    assert_eq!(rate_text(56_500.), "56.5 KB/s");
    assert_eq!(rate_text(2_100_000.), "2.1 MB/s");
    assert_eq!(rate_text(3.5e9), "3.5 GB/s");
}

/// A ready snapshot whose sample may carry the negotiated network field.
fn sample_state(with_network: bool) -> State {
    let mut sample = serde_json::json!({
        "cpu_percent": 12.0,
        "memory_used_bytes": 8,
        "memory_total_bytes": 16,
        "sampled_at_millis": 1_700_000_000_000_u64,
    });
    if with_network {
        sample["network"] = serde_json::json!({
            "rx_bytes_per_sec": 2_100_000.0,
            "tx_bytes_per_sec": 240_000.0,
        });
    }
    State::Ready(
        serde_json::from_value(serde_json::json!({
            "hostname": "metrics-network-test",
            "instance": "metrics-network-instance",
            "state": {"sample": sample},
        }))
        .unwrap(),
    )
}

#[test]
fn network_rates_render_per_state_and_never_as_quiet_zeros() {
    // A supporting peer with a reading: both rates formatted, no state word,
    // and the hover detail names the host with both values.
    let ready = readout(&sample_state(true), true);
    assert!(ready.network_ready());
    assert_eq!(ready.network_rx_text(), "↓ 2.1 MB/s");
    assert_eq!(ready.network_tx_text(), "↑ 240 KB/s");
    assert_eq!(ready.network_status, None);
    assert!(
        ready.network_detail.contains("metrics-network-test")
            && ready.network_detail.contains("2.1 MB/s")
            && ready.network_detail.contains("240 KB/s"),
        "the hover detail carries host and both rates: {}",
        ready.network_detail
    );

    // Supporting peer, reading missing (fresh baseline, counter reset): the
    // waiting family, explicitly not a zero.
    let waiting = readout(&sample_state(false), true);
    assert!(!waiting.network_ready());
    assert_eq!(waiting.network_rx_text(), "↓ …");
    assert_eq!(waiting.network_tx_text(), "↑ …");
    assert_eq!(waiting.network_status, Some(t(L10nKey::HostMetricsWaiting)));

    // A peer without the capability never carries the field: unsupported,
    // not an endless wait.
    let unsupported = readout(&sample_state(false), false);
    assert!(!unsupported.network_ready());
    assert_eq!(unsupported.network_rx_text(), "↓ —");
    assert_eq!(unsupported.network_tx_text(), "↑ —");
    assert_eq!(
        unsupported.network_status,
        Some(t(L10nKey::HostMetricsUnavailable))
    );

    // Whole-sampler states drag both series into their marker family.
    let pending = readout(&State::Connecting, false);
    assert_eq!(pending.network_rx_text(), "↓ …");
    let down = readout(&State::Disconnected, true);
    assert_eq!(down.network_rx_text(), "↓ —");
    assert_eq!(
        down.network_status,
        Some(t(L10nKey::HostMetricsDisconnected))
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
fn titlebar_metrics_track_the_context_host_and_click_opens_monitor(cx: &mut TestAppContext) {
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
    let hostname =
        std::thread::spawn(move || server.client.host_metrics().unwrap().hostname().to_owned())
            .join()
            .unwrap();

    // Both metrics render with the host's own reading in their detail.
    vcx.update(|window, _| {
        for id in [
            "titlebar-metric-cpu",
            "titlebar-metric-ram",
            "titlebar-metric-gpu",
        ] {
            let metric = window.find(id);
            assert!(metric.visible(), "{id} must render in the chrome");
            assert!(
                metric.label().unwrap().contains(&hostname),
                "{id} must name the context pane's host"
            );
        }
        assert!(window.find("titlebar-metric-cpu-chart").visible());
        assert!(window.find("titlebar-metric-ram-chart").visible());
        assert!(window.find("titlebar-metric-gpu-chart").visible());
        // The three metrics share the approved two-line capsule: readings on
        // the upper line, short trends below, sitting right of the centered
        // Park entry and left of the command group.
        let capsule = window.find("titlebar-metrics").bounds();
        assert!(
            (capsule.size.width - px(204.)).abs() <= px(1.),
            "the reviewed capsule is approximately 204 px at the default scale"
        );
        assert!(window.find("titlebar-metric-readings").visible());
        assert!(window.find("titlebar-metric-trends").visible());
        assert!(window.find("titlebar-park").bounds().right() <= capsule.left());
        assert!(window.find("titlebar-split-right").bounds().left() >= capsule.right());
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
        assert!(window.find("monitor-tile-gpu").visible());
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
fn monitor_network_tile_groups_receive_and_send_on_the_shared_samples(cx: &mut TestAppContext) {
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

    // The real daemon negotiates the network capability, and its genuine
    // sample carries off-host rates (possibly idle zeros, never absent).
    let client = server.client.clone();
    let (hostname, wire) = std::thread::spawn(move || {
        let metrics = client.host_metrics().unwrap();
        let hostname = metrics.hostname().to_owned();
        let wire = match metrics.state() {
            MetricsState::Sample(sample) => sample
                .network()
                .map(|network| (network.rx_bytes_per_sec(), network.tx_bytes_per_sec())),
            other => panic!("the wired sample never arrived: {other:?}"),
        };
        (hostname, wire)
    })
    .join()
    .unwrap();
    let wire = wire.expect("the negotiated sample carries a network reading");
    assert!(
        wire.0.is_finite() && wire.0 >= 0. && wire.1.is_finite() && wire.1 >= 0.,
        "genuine off-host rates: {wire:?}"
    );

    vcx.update(|window, cx| window.click("titlebar-metric-cpu", cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(window.find("monitor-tile-cpu").visible());
        assert!(
            window.find("monitor-tile-network").visible(),
            "the network tile renders in the resource section"
        );
        assert!(
            window
                .find("monitor-tile-network")
                .label()
                .unwrap()
                .contains(&hostname),
            "the network tile names the context pane's host"
        );
        // Both series are on screen with rate formatting, and the overlay
        // chart is present because a genuine reading exists.
        let rx = window
            .find("monitor-tile-network-rx")
            .label()
            .unwrap()
            .to_owned();
        let tx = window
            .find("monitor-tile-network-tx")
            .label()
            .unwrap()
            .to_owned();
        assert!(rx.starts_with('↓') && rx.ends_with("/s"), "rx reads {rx:?}");
        assert!(tx.starts_with('↑') && tx.ends_with("/s"), "tx reads {tx:?}");
        assert!(window.find("monitor-tile-network-chart").visible());
        // The distinct waiting/unsupported words stay off screen while a
        // real reading is present.
        assert_ne!(
            window.find("monitor-tile-network").label(),
            Some(i18n::t(L10nKey::HostMetricsUnavailable))
        );
    });
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

/// The narrowing order, exercised through a real window: microcharts first,
/// then the Park word, then the whole capsule. Widths come from the same
/// public budget the product derives its thresholds from, so both platforms
/// exercise the same stages however their native controls differ.
#[gpui_kit::test]
fn narrow_windows_drop_charts_then_compress_park_then_hide_the_capsule(cx: &mut TestAppContext) {
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

    let rem = 16.;
    use crate::ui::window_chrome::{
        PARK_ENTRY_COMPACT_W, PARK_ENTRY_FULL_W, titlebar_native_controls_width,
        titlebar_required_width,
    };
    let controls = vcx.update(|window, _| titlebar_native_controls_width(window));
    let full_w = titlebar_required_width(
        rem,
        false,
        controls,
        PARK_ENTRY_FULL_W,
        capsule_strip_w(3, true, rem),
    );
    let no_charts_w = titlebar_required_width(
        rem,
        false,
        controls,
        PARK_ENTRY_FULL_W,
        capsule_strip_w(3, false, rem),
    );
    let compact_park_w = titlebar_required_width(
        rem,
        false,
        controls,
        PARK_ENTRY_COMPACT_W,
        capsule_strip_w(3, false, rem),
    );

    // Wide: the full two-line capsule sits right of the labelled Park entry.
    vcx.simulate_resize(size(px(full_w + 40.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        let capsule = window.find("titlebar-metrics").bounds();
        let entry = window.find("titlebar-park").bounds();
        assert!(
            (capsule.size.width - px(204.)).abs() <= px(1.),
            "the reviewed capsule is approximately 204 px at the default scale"
        );
        assert!(window.find("titlebar-metric-readings").visible());
        assert!(window.find("titlebar-metric-trends").visible());
        assert!(
            (window.find("titlebar-park").bounds().size.width - px(108.)).abs() <= px(1.),
            "the wide entry keeps its labelled form"
        );
        assert!(capsule.left() >= entry.right());
        assert!(window.find("titlebar-split-right").bounds().left() >= capsule.right());
        assert!(window.find("titlebar-add").bounds().right() <= entry.left());
    });

    // Microcharts no longer fit: the readings survive, Park keeps its word.
    vcx.simulate_resize(size(px(no_charts_w + 8.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        let capsule = window.find("titlebar-metrics").bounds();
        assert!(
            (capsule.size.width - px(168.)).abs() <= px(1.),
            "readings-only is the fallback the narrowing order buys first"
        );
        assert!(window.try_find("titlebar-metric-trends").is_none());
        assert!(window.try_find("titlebar-metric-cpu-chart").is_none());
        assert!(window.find("titlebar-metric-cpu-value").visible());
        assert!(window.find("titlebar-metric-gpu-value").visible());
        assert!(
            (window.find("titlebar-park").bounds().size.width - px(108.)).abs() <= px(1.),
            "losing the microcharts cannot move the Park entry yet"
        );
        assert!(capsule.left() >= window.find("titlebar-park").bounds().right());
    });

    // Park compresses to icon + count next; the capsule stays readable.
    vcx.simulate_resize(size(px(compact_park_w + 8.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        let entry = window.find("titlebar-park");
        assert!(
            (entry.bounds().size.width - px(56.)).abs() <= px(1.),
            "the compressed entry is the icon+count pill"
        );
        assert!(window.find("titlebar-metrics").visible());
        assert!(window.find("titlebar-metrics").bounds().left() >= entry.bounds().right());
    });

    // Narrowest: the capsule hides entirely; the compact entry stays, and
    // the chrome around it never yields.
    vcx.simulate_resize(size(px(compact_park_w - 40.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(window.try_find("titlebar-metrics").is_none());
        assert!(window.find("titlebar-park").visible());
        assert!(
            window.find("workbench-left-drag").bounds().size.width >= px(40.),
            "the drag band survives at the narrowest stage"
        );
        for id in [
            "titlebar-split-right",
            "titlebar-split-down",
            "titlebar-right-panel",
        ] {
            assert!(window.find(id).visible(), "{id}");
        }
    });

    // The budget is in rems: a larger interface reaches the same stages
    // earlier than the default rem would at the same pixel width.
    app.update_in(&mut vcx, |_, _, cx| {
        cx.global_mut::<lyrel_ui_base::config::Config>()
            .ui_font_size = 20.;
        cx.notify();
    });
    let rem = 20.;
    let scaled_compact = titlebar_required_width(
        rem,
        false,
        controls,
        PARK_ENTRY_COMPACT_W * (rem / 16.),
        capsule_strip_w(3, false, rem),
    );
    vcx.simulate_resize(size(px(scaled_compact + 30.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(
            window.try_find("titlebar-metrics").is_some(),
            "a window just wider than the scaled compact need keeps readings"
        );
        assert!(
            window.try_find("titlebar-metric-cpu-chart").is_none(),
            "charts do not fit at this rem either"
        );
    });
    vcx.simulate_resize(size(px(scaled_compact - 30.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(
            window.try_find("titlebar-metrics").is_none(),
            "a window just narrower than the scaled compact need hides the capsule"
        );
    });
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
    settle(&mut vcx);
    vcx.simulate_resize(size(px(1440.), px(720.)));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(window.find("titlebar-metric-cpu").visible());
        assert!(window.find("titlebar-metric-ram").visible());
        let metrics = window.find("titlebar-metrics").bounds();
        assert!(metrics.right() <= window.viewport_size().width);
        assert!(metrics.top() >= px(0.));
    });
    vcx.simulate_resize(size(px(760.), px(720.)));
    app.update_in(&mut vcx, |_, _, cx| {
        cx.global_mut::<lyrel_ui_base::config::Config>()
            .ui_font_size = 20.;
        cx.notify();
    });
    settle(&mut vcx);
    vcx.update(|window, _| assert!(window.try_find("titlebar-metrics").is_none()));
    vcx.simulate_resize(size(px(1440.), px(720.)));
    wait_sample(&mut vcx, true);

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

/// The capsule width hands the chrome budget: three metrics buy the reviewed
/// ~204 px two-line capsule (plus its trailing gap), fewer buy proportional
/// widths, and the readings-only fallback is always narrower than the full
/// capsule for the same count.
#[test]
fn capsule_strip_w_scales_with_the_enabled_count() {
    assert_eq!(capsule_strip_w(3, true, 16.), 216.);
    assert_eq!(capsule_strip_w(3, false, 16.), 180.);
    assert_eq!(capsule_strip_w(3, true, 20.), 270.);
    assert!(capsule_strip_w(1, true, 16.) < capsule_strip_w(3, true, 16.));
    assert!(capsule_strip_w(2, true, 16.) < capsule_strip_w(3, true, 16.));
    assert!(capsule_strip_w(3, false, 16.) < capsule_strip_w(3, true, 16.));
    assert!(capsule_strip_w(2, false, 16.) < capsule_strip_w(2, true, 16.));
    assert!(capsule_strip_w(1, false, 16.) < capsule_strip_w(1, true, 16.));
}

#[test]
fn titlebar_ram_and_monitor_pair_carry_their_unit() {
    let gib = 1_073_741_824_u64;
    let state = State::Ready(HostMetrics::new(
        "metrics-host",
        "metrics-daemon",
        MetricsState::Sample(HostSample::new(
            12.,
            (13.1_f64 * gib as f64) as u64,
            32 * gib,
            1_700_000_000_000,
        )),
    ));
    let reading = readout(&state, false);
    assert_eq!(reading.memory_text(), "13.1 GB");
    assert_eq!(reading.memory_total_text().as_deref(), Some("/ 32 GB"));
}
