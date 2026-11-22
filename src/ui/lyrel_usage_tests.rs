//! Consumer regressions for the Lyrel component-usage row (#377) through the
//! production workbench and real Host servers, plus the identity rules that
//! keep a replaced daemon from replaying its predecessor's readings.

use super::*;
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::daemon::control::ControlHello;
use lyrel_core::host::lyrel_usage::LyrelSample;
use lyrel_ui_base::i18n;
use lyrel_ui_base::session::{RemoteTarget, WorkspaceId};

use crate::ui::app::test_window;
use crate::ui::desktop_lookups::HostLinks;

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
    // The poll lands after the frame that armed it; one more frame paints
    // the landed sample before assertions read the row.
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

/// Whether the collapsed summary currently shows a real CPU percentage.
fn has_lyrel_sample(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, _| {
        window
            .try_find("lyrel-usage-cpu")
            .and_then(|cpu| cpu.label().map(str::to_owned))
            .is_some_and(|label| label.ends_with('%'))
    })
}

fn wait_lyrel_sample(vcx: &mut VisualTestContext, wanted: bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(vcx);
        if has_lyrel_sample(vcx) == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Lyrel sample availability did not become {wanted}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn open_monitor(app: &gpui_kit::Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| {
        app.set_right_panel_tab(RightPanelTab::Monitor, window, cx);
    });
    settle(vcx);
}

/// The real-daemon fixture over TCP, serving the full local Host — the same
/// `HostServer` the host-metrics consumer tests use.
fn local_server_standalone() -> crate::ui::host_metrics::tests::HostServer {
    crate::ui::host_metrics::tests::HostServer::new()
}

/// A real remote Lyrel server: the same control seam a managed remote's
/// daemon answers over, with its own process behind the wire sample.
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
            &ControlHello::host_rpc("lyrel-usage-remote", "consumer"),
        )
        .unwrap();
        Self {
            host,
            thread: Some(thread),
        }
    }

    fn install(&self, vcx: &mut VisualTestContext) {
        vcx.update(|_, cx| {
            HostLinks::insert(cx, self.host.clone(), self.host.home());
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
fn monitor_lyrel_row_collapses_summarizes_and_expands_by_host(cx: &mut TestAppContext) {
    let server = local_server_standalone();
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| {
        crate::ui::local_link::LocalLink::install_client_for_test(cx, server.client.clone())
    });
    app.update_in(&mut vcx, |app, _, _| app.right_panel_width.set(276.));
    open_monitor(&app, &mut vcx);

    // Collapsed by default: the summary line names the local aggregate and
    // nothing is expanded yet.
    wait_lyrel_sample(&mut vcx, true);
    vcx.update(|window, _| {
        assert!(window.find("lyrel-usage-summary").visible());
        assert!(
            window
                .find("lyrel-usage-cpu")
                .label()
                .unwrap()
                .starts_with("CPU ")
        );
        assert!(
            window
                .find("lyrel-usage-ram")
                .label()
                .unwrap()
                .starts_with("RAM ")
        );
        assert!(
            window
                .find("lyrel-usage-gpu")
                .label()
                .unwrap()
                .starts_with("GPU ")
        );
        assert!(window.try_find("lyrel-usage-desktop").is_none());
        assert!(window.try_find("lyrel-usage-group-remote").is_none());
        assert!(window.try_find("lyrel-usage-includes-remote").is_none());
    });

    // The daemon half of the summary came over the wire stamped with this
    // very daemon's instance — the row accepted it. Ask off the UI thread,
    // the same rule production callers follow.
    let probe = server.client.clone();
    let (wire_instance, wire_hostname) = std::thread::spawn(move || {
        let metrics = probe.host_metrics().unwrap();
        let usage = metrics
            .lyrel_usage()
            .map(|usage| usage.instance().to_owned());
        (usage, metrics.hostname().to_owned())
    })
    .join()
    .unwrap();
    assert_eq!(
        wire_instance.as_deref(),
        Some(server.client.hello().instance.as_str()),
        "the fixture daemon did not stamp its Lyrel usage with its instance"
    );
    assert!(!wire_hostname.is_empty());

    vcx.update(|window, cx| window.click("lyrel-usage-summary", cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        // The local group lists exactly the two Lyrel components: the desktop
        // application and the session daemon. A pane shell or agent is a
        // child of the daemon's workload, never a Lyrel component, so no
        // third local row exists.
        let local = window.find("lyrel-usage-group-local");
        assert!(local.visible());
        let desktop = window.find("lyrel-usage-desktop");
        let daemon = window.find("lyrel-usage-daemon");
        assert!(
            desktop
                .label()
                .is_some_and(|label| label.starts_with(i18n::t(L10nKey::LyrelUsageDesktop))),
            "desktop row: {:?}",
            desktop.label()
        );
        assert!(
            daemon
                .label()
                .is_some_and(|label| label.starts_with(i18n::t(L10nKey::LyrelUsageDaemon))),
            "daemon row: {:?}",
            daemon.label()
        );
        assert!(
            window
                .find("lyrel-usage-desktop-cpu")
                .label()
                .unwrap()
                .ends_with('%')
        );
        assert!(
            window
                .find("lyrel-usage-daemon-cpu")
                .label()
                .unwrap()
                .ends_with('%')
        );
        assert!(window.find("lyrel-usage-daemon-mem").label().is_some());
        for (gpu_id, network_id) in [
            ("lyrel-usage-desktop-gpu", "lyrel-usage-desktop-network"),
            ("lyrel-usage-daemon-gpu", "lyrel-usage-daemon-network"),
        ] {
            let gpu = window.find(gpu_id);
            assert!(gpu.label().unwrap().starts_with("GPU"));
            let network = window.find(network_id);
            let label = network.label().unwrap();
            #[cfg(not(target_os = "macos"))]
            assert_eq!(
                label,
                unavailable_detail(
                    t(L10nKey::MonitorColNetwork),
                    Some(ProcessMetricUnavailable::Unsupported)
                )
            );
            #[cfg(target_os = "macos")]
            assert!(
                label.starts_with(t(L10nKey::LyrelProcessNetwork))
                    || label.starts_with(t(L10nKey::MonitorColNetwork))
            );
        }
        assert!(window.try_find("lyrel-usage-server").is_none());
    });

    // The desktop row measures this very process.
    let desktop_pid = app.read_with(&vcx, |app, cx| {
        let view = app.lyrel_usage_view.as_ref().expect("the view is held");
        view.read(cx).desktop.clone()
    });
    let Desktop::Ready(component) = desktop_pid else {
        panic!("the desktop sample never landed: {desktop_pid:?}");
    };
    assert_eq!(component.pid(), std::process::id());

    // Losing the daemon clears the numbers — no cached or local-process
    // fallback stands in for the wire reading.
    server.client.close();
    wait_lyrel_sample(&mut vcx, false);
    vcx.update(|window, _| {
        assert_eq!(window.find("lyrel-usage-cpu").label(), Some("CPU —"));
    });

    // A replacement daemon is a new instance: its samples replace the old
    // ones rather than inheriting them.
    let reconnected = local_server_standalone();
    vcx.update(|_, cx| {
        crate::ui::local_link::LocalLink::install_client_for_test(cx, reconnected.client.clone())
    });
    wait_lyrel_sample(&mut vcx, true);
}

#[gpui_kit::test]
fn remote_workspace_lists_the_lyrel_server_under_its_own_host(cx: &mut TestAppContext) {
    let local = local_server_standalone();
    let target = RemoteTarget::Alias {
        alias: "lyrel-usage-route".into(),
    };
    let remote = RemoteServer::new(&target);
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| {
        crate::ui::local_link::LocalLink::install_client_for_test(cx, local.client.clone())
    });
    remote.install(&mut vcx);
    // Point the context pane at the remote route: the monitor's host becomes
    // the remote one while the window keeps its local daemon link.
    app.update_in(&mut vcx, |app, window, cx| {
        let leaf = app.tabs[app.tabs.active_index()]
            .detail_pane(window, cx)
            .expect("the harness tab has a pane");
        leaf.update(cx, |view, _| {
            view.set_workspace(Some(lyrel_ui_base::route::PaneWorkspace {
                workspace: WorkspaceId::new(),
                target: target.clone(),
                spec: None,
                label: Some("lyrel-usage-route".into()),
                resize_echo: false,
            }));
        });
    });
    app.update_in(&mut vcx, |app, _, _| app.right_panel_width.set(276.));
    open_monitor(&app, &mut vcx);
    wait_lyrel_sample(&mut vcx, true);

    vcx.update(|window, cx| window.click("lyrel-usage-summary", cx));
    settle(&mut vcx);
    let probe = remote.host.clone();
    let remote_hostname = std::thread::spawn(move || {
        let metrics = probe.host_metrics().unwrap();
        metrics
            .lyrel_usage()
            .map(|usage| usage.hostname().to_owned())
            .unwrap_or_default()
    })
    .join()
    .unwrap();
    assert!(
        !remote_hostname.is_empty(),
        "the remote fixture daemon reported no hostname"
    );
    vcx.update(|window, _| {
        // Local and remote stay separated: the desktop application and the
        // session daemon under the local host, the Lyrel server under the
        // remote host that actually measured it.
        assert!(window.find("lyrel-usage-group-local").visible());
        let group = window.find("lyrel-usage-group-remote");
        assert!(group.visible());
        let group_label = group.label().unwrap_or_default();
        assert!(
            group_label.contains(&remote_hostname),
            "the remote group is not named after the measuring host: {group_label}"
        );
        let server_row = window.find("lyrel-usage-server");
        assert!(
            server_row
                .label()
                .is_some_and(|label| label.starts_with(i18n::t(L10nKey::LyrelUsageServer))),
            "server row: {:?}",
            server_row.label()
        );
        assert!(
            window
                .find("lyrel-usage-server-cpu")
                .label()
                .unwrap()
                .ends_with('%')
        );
        assert!(window.find("lyrel-usage-server-mem").label().is_some());
        // The collapsed summary carries the "includes remote" badge.
        assert!(window.try_find("lyrel-usage-includes-remote").is_some());
    });

    // The remote Lyrel server disappearing turns its row honest ("—") while
    // the local components keep their live readings — no cross-host reuse.
    remote.host.client().close();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
        let remote_down = vcx.update(|window, _| {
            window.try_find("lyrel-usage-server-cpu").is_none()
                || window.find("lyrel-usage-server-cpu").label() == Some("—")
        });
        if remote_down {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the disconnected remote server kept its old readings"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    vcx.update(|window, _| {
        assert_eq!(window.find("lyrel-usage-server-cpu").label(), Some("—"));
        assert!(
            window
                .find("lyrel-usage-desktop-cpu")
                .label()
                .unwrap()
                .ends_with('%')
        );
        assert!(
            window
                .find("lyrel-usage-daemon-cpu")
                .label()
                .unwrap()
                .ends_with('%')
        );
        assert!(window.try_find("lyrel-usage-includes-remote").is_none());
    });
}

#[test]
fn a_sample_from_another_daemon_instance_is_dropped_to_priming() {
    let stamp = LyrelUsage::new(
        "fixture-host",
        "previous-instance",
        LyrelUsageState::Sample(LyrelSample::new(
            42,
            ComponentUsage::new(4242, 1_700_000_000, 1.5, 96 * 1024 * 1024),
        )),
    );
    let mut metrics = HostMetrics::new(
        "fixture-host",
        "current-instance",
        lyrel_core::host::metrics::MetricsState::Waiting,
    );
    metrics.set_lyrel_usage(Some(stamp));
    assert_eq!(
        slot_of(Ok(metrics), "current-instance"),
        Slot::Priming,
        "a previous daemon's stamped sample must not present as current"
    );

    let supported_but_absent = HostMetrics::new(
        "fixture-host",
        "current-instance",
        lyrel_core::host::metrics::MetricsState::Waiting,
    );
    assert_eq!(
        slot_of(Ok(supported_but_absent), "current-instance"),
        Slot::Unsupported,
        "a peer that never attached the field cannot attribute Lyrel usage"
    );

    assert_eq!(
        slot_of(
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no sampler"
            )),
            "current-instance"
        ),
        Slot::Unsupported
    );
    assert_eq!(
        slot_of(
            Err(std::io::Error::from(std::io::ErrorKind::NotConnected)),
            "current-instance"
        ),
        Slot::Gone
    );
}

#[test]
fn network_pair_shares_units_only_when_they_match() {
    assert_eq!(network_pair_text(52_000., 8_000.), "52 / 8 KB/s");
    assert_eq!(network_pair_text(52_000., 8.), "52 KB/s / 8 B/s");
}

#[test]
fn the_collapsed_aggregate_sums_only_real_pairs() {
    let ready = Cell {
        cpu: Some(1.2),
        memory: Some(210 * 1024 * 1024),
        pending: false,
    };
    let other = Cell {
        cpu: Some(0.6),
        memory: Some(74 * 1024 * 1024),
        pending: false,
    };
    let total = ready.add(other);
    assert!(
        (total.cpu.unwrap_or_default() - 1.8).abs() < 1e-5,
        "cpu sum: {:?}",
        total.cpu
    );
    assert_eq!(total.memory, Some(284 * 1024 * 1024));

    let pending = Cell::pending();
    assert!(
        ready.add(pending).pending,
        "one pending half keeps the sum pending instead of half a number"
    );
    let down = Cell::down();
    assert_eq!(
        ready.add(down).cpu,
        None,
        "one down half makes the aggregate honestly unavailable"
    );
    assert!(!ready.add(down).pending);
}
