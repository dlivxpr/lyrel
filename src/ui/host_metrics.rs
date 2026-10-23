//! Built-in CPU/RAM presentation. One retained sampler view per workspace host is
//! shared by windows; its notifications never depend on terminal output.
use std::collections::HashMap;
use std::time::Duration;
use std::sync::Arc;

use gpui_kit::component::{h_flex, tooltip::Tooltip};
use gpui_kit::{App, Context, Entity, Global, IntoElement, Render, WeakEntity, Window, div, prelude::*, rems};
use gpui_kit::TestSupportExt as _;
use lyrel_core::host::{Host as _, HostId, remote::RemoteHost, metrics::{HostMetrics, MetricsState}};

use crate::ui::app::LyrelApp;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::local_link::LocalLink;
use crate::ui::remote_connect::HostLinks;
use crate::ui::remote_workspace::{MachineStatus, RemoteLinks};

#[derive(Clone)]
enum MetricsSource {
    Local(Arc<lyrel_core::daemon::control::ControlClient>),
    Remote(Arc<RemoteHost>),
}

impl MetricsSource {
    fn get(host: HostId, cx: &mut App) -> Option<Self> {
        if host.is_local() {
            LocalLink::client(cx).map(Self::Local)
        } else {
            HostLinks::get(cx, host).map(Self::Remote)
        }
    }

    fn sample(&self) -> std::io::Result<HostMetrics> {
        match self {
            Self::Local(client) => client.host_metrics(),
            Self::Remote(host) => host.host_metrics(),
        }
    }

    fn is_current(&self, host: HostId, cx: &mut App) -> bool {
        match (self, Self::get(host, cx)) {
            (Self::Local(old), Some(Self::Local(current))) => Arc::ptr_eq(old, &current),
            (Self::Remote(old), Some(Self::Remote(current))) => Arc::ptr_eq(old, &current),
            _ => false,
        }
    }

    fn instance(&self) -> &str {
        match self {
            Self::Local(client) => &client.hello().instance,
            Self::Remote(host) => &host.peer().instance,
        }
    }

    fn is_connected(&self) -> bool {
        match self {
            Self::Local(client) => client.is_connected(),
            Self::Remote(host) => host.client().is_connected(),
        }
    }
}

#[derive(Default)]
struct MetricsViews(HashMap<HostId, WeakEntity<HostMetricsView>>);
impl Global for MetricsViews {}

#[derive(Clone, PartialEq)]
enum State {
    Waiting,
    Connecting,
    Reconnecting,
    Ready(HostMetrics),
    Disconnected,
    Unsupported,
    Failed(String),
}

fn missing_source(host: HostId, cx: &App) -> State {
    match RemoteLinks::machine_status(cx, host) {
        Some(MachineStatus::Connecting) => State::Connecting,
        Some(MachineStatus::Reconnecting { .. }) => State::Reconnecting,
        _ => State::Disconnected,
    }
}

pub(crate) struct HostMetricsView {
    host: HostId,
    state: State,
}

impl HostMetricsView {
    fn acquire(host: HostId, cx: &mut App) -> Entity<Self> {
        if let Some(view) = cx.default_global::<MetricsViews>().0.get(&host)
            .and_then(WeakEntity::upgrade)
        {
            return view;
        }
        let view = cx.new(|cx| Self::new(host, cx));
        let views = &mut cx.default_global::<MetricsViews>().0;
        views.retain(|_, view| view.upgrade().is_some());
        views.insert(host, view.downgrade());
        view
    }

    fn new(host: HostId, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                if this.upgrade().is_none() { break; }
                let source = cx.update(|cx| MetricsSource::get(host, cx));
                let next = match source {
                    Some(source) => {
                        let query = source.clone();
                        let result = cx.background_executor().spawn(async move { query.sample() }).await;
                        if !cx.update(|cx| source.is_current(host, cx)) {
                            if this.update(cx, |view, cx| {
                                view.state = missing_source(host, cx);
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
                                | std::io::ErrorKind::UnexpectedEof) => cx.update(|cx| missing_source(host, cx)),
                            Err(error) => State::Failed(error.to_string()),
                        }
                    }
                    None => cx.update(|cx| missing_source(host, cx)),
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
        Self { host, state: if host.is_local() { State::Waiting } else { State::Connecting } }
    }
}

impl Render for HostMetricsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A shared view can outlive a connection. Reject its previous daemon's
        // sample before the next poll when the route was replaced or went down.
        let replacement = if let State::Ready(snapshot) = &self.state {
            match MetricsSource::get(self.host, cx) {
                Some(source) if source.is_connected() && source.instance() == snapshot.instance() => None,
                Some(source) if source.is_connected() => Some(State::Waiting),
                _ => Some(missing_source(self.host, cx)),
            }
        } else { None };
        let state = replacement.as_ref().unwrap_or(&self.state);
        let (cpu, memory, detail) = match state {
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
            State::Connecting => ("…".into(), "…".into(), t(L10nKey::HostMetricsConnecting).to_owned()),
            State::Reconnecting => ("—".into(), "—".into(), t(L10nKey::HostMetricsReconnecting).to_owned()),
            State::Disconnected => ("—".into(), "—".into(), t(L10nKey::HostMetricsDisconnected).to_owned()),
            State::Unsupported => ("—".into(), "—".into(), t(L10nKey::HostMetricsUnavailable).to_owned()),
            State::Failed(reason) => ("!".into(), "!".into(), t_fmt(L10nKey::HostMetricsFailed, &[("reason", reason)])),
        };
        let status = match state {
            State::Waiting => Some(t(L10nKey::HostMetricsWaiting)),
            State::Connecting => Some(t(L10nKey::HostMetricsConnecting)),
            State::Reconnecting => Some(t(L10nKey::HostMetricsReconnecting)),
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
        let host = self.spawn_host(cx);
        if self.status_document_context(window, cx).is_some() {
            self.status_metrics = None;
        } else if self.status_metrics.as_ref().is_none_or(|view| view.read(cx).host != host) {
            self.status_metrics = Some(HostMetricsView::acquire(host, cx));
        }
    }

    pub(crate) fn render_host_metrics(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if let Some(view) = &self.status_metrics
            && view.read(cx).host == self.spawn_host(cx)
        {
            return view.clone().into_any_element();
        }
        metrics_row("…".into(), "…".into(), t(L10nKey::HostMetricsWaiting).to_owned(),
            Some(t(L10nKey::HostMetricsWaiting))).into_any_element()
    }
}
