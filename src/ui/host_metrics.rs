//! Built-in CPU/RAM presentation. One retained sampler view per local host is
//! shared by windows; its notifications never depend on terminal output.
use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::{h_flex, tooltip::Tooltip};
use gpui_kit::{App, Context, Entity, Global, IntoElement, Render, WeakEntity, Window, div, prelude::*, rems};
use gpui_kit::TestSupportExt as _;
use lyrel_core::host::{HostId, metrics::{HostMetrics, MetricsState}};

use crate::ui::app::LyrelApp;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::local_link::LocalLink;

#[derive(Default)]
struct MetricsViews(HashMap<HostId, WeakEntity<HostMetricsView>>);
impl Global for MetricsViews {}

#[derive(Clone, PartialEq)]
enum State {
    Waiting,
    Ready(HostMetrics),
    Disconnected,
    Unsupported,
    Failed(String),
}

pub(crate) struct HostMetricsView {
    state: State,
}

impl HostMetricsView {
    fn acquire(cx: &mut App) -> Entity<Self> {
        if let Some(view) = cx.default_global::<MetricsViews>().0.get(&HostId::LOCAL)
            .and_then(WeakEntity::upgrade)
        {
            return view;
        }
        let view = cx.new(Self::new);
        cx.default_global::<MetricsViews>().0.insert(HostId::LOCAL, view.downgrade());
        view
    }

    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                if this.upgrade().is_none() { break; }
                let client = cx.update(LocalLink::client);
                let next = match client {
                    Some(client) => {
                        let instance = client.hello().instance.clone();
                        let result = cx.background_executor().spawn(async move { client.host_metrics() }).await;
                        let current = cx.update(|cx| LocalLink::client(cx)
                            .is_some_and(|client| client.hello().instance == instance));
                        if !current {
                            if this.update(cx, |view, cx| {
                                view.state = State::Disconnected;
                                cx.notify();
                            }).is_err() { break; }
                            cx.background_executor().timer(Duration::from_secs(1)).await;
                            continue;
                        }
                        match result {
                            Ok(snapshot) => State::Ready(snapshot),
                            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => State::Unsupported,
                            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotConnected
                                | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::UnexpectedEof) => State::Disconnected,
                            Err(error) => State::Failed(error.to_string()),
                        }
                    }
                    None => State::Disconnected,
                };
                if this.update(cx, |view, cx| {
                    if view.state != next {
                        view.state = next;
                        cx.notify();
                    }
                }).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        }).detach();
        Self { state: State::Waiting }
    }
}

impl Render for HostMetricsView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (cpu, memory, detail) = match &self.state {
            State::Ready(snapshot) => match snapshot.state() {
                MetricsState::Sample(sample) => (
                    format!("{:.0}%", sample.cpu_percent()),
                    format!("{:.0}%", sample.memory_percent()),
                    t_fmt(L10nKey::HostMetricsSample, &[
                        ("host", snapshot.hostname()),
                        ("used", &format!("{:.1}", sample.memory_used_bytes() as f64 / 1_073_741_824.0)),
                        ("total", &format!("{:.1}", sample.memory_total_bytes() as f64 / 1_073_741_824.0)),
                    ]),
                ),
                MetricsState::Waiting => ("…".into(), "…".into(), t(L10nKey::HostMetricsWaiting).to_owned()),
                MetricsState::Unavailable(reason) => ("—".into(), "—".into(),
                    t_fmt(L10nKey::HostMetricsFailed, &[("reason", reason)])),
            },
            State::Waiting => ("…".into(), "…".into(), t(L10nKey::HostMetricsWaiting).to_owned()),
            State::Disconnected => ("—".into(), "—".into(), t(L10nKey::HostMetricsDisconnected).to_owned()),
            State::Unsupported => ("—".into(), "—".into(), t(L10nKey::HostMetricsUnavailable).to_owned()),
            State::Failed(reason) => ("!".into(), "!".into(), t_fmt(L10nKey::HostMetricsFailed, &[("reason", reason)])),
        };
        let status = match &self.state {
            State::Waiting => Some(t(L10nKey::HostMetricsWaiting)),
            State::Disconnected => Some(t(L10nKey::HostMetricsDisconnected)),
            State::Unsupported => Some(t(L10nKey::HostMetricsUnavailable)),
            State::Failed(_) => Some(t(L10nKey::HostMetricsError)),
            State::Ready(snapshot) => match snapshot.state() {
                MetricsState::Waiting => Some(t(L10nKey::HostMetricsWaiting)),
                MetricsState::Unavailable(_) => Some(t(L10nKey::HostMetricsError)),
                MetricsState::Sample(_) => None,
            },
        };
        metrics_row(cpu, memory, detail, status)
    }
}

fn metrics_row(cpu: String, memory: String, detail: String, status: Option<&'static str>) -> impl IntoElement {
    let cpu = format!("CPU {cpu}");
    let memory = format!("MEM {memory}");
    h_flex()
        .id("status-host-metrics")
        .test_support()
        .aria_label(detail.clone())
        .flex_none()
        .gap_2()
        .px_1()
        .child(div().id("status-cpu").test_support().aria_label(cpu.clone()).child(cpu))
        .child(div().id("status-memory").test_support().aria_label(memory.clone()).child(memory))
        .when_some(status, |row, status| row.child(div().max_w(rems(9.)).text_ellipsis().child(status)))
        .tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
}

impl LyrelApp {
    pub(crate) fn sync_status_metrics(&mut self, window: &Window, cx: &mut Context<Self>) {
        let wanted = self.status_document_context(window, cx).is_none() && self.spawn_host(cx).is_local();
        if wanted && self.status_metrics.is_none() {
            self.status_metrics = Some(HostMetricsView::acquire(cx));
        } else if !wanted {
            self.status_metrics = None;
        }
    }

    pub(crate) fn render_host_metrics(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if let Some(view) = &self.status_metrics {
            return view.clone().into_any_element();
        }
        let connected = self.active_host(cx).is_some_and(|host| host.is_connected());
        let status = t(if connected {
            L10nKey::HostMetricsUnavailable
        } else {
            L10nKey::HostMetricsDisconnected
        });
        metrics_row("—".into(), "—".into(), status.to_owned(), Some(status)).into_any_element()
    }
}
