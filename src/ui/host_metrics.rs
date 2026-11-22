//! Shared whole-host CPU/RAM/network sampling for the title bar and Activity
//! Monitor (#373, #375).
//!
//! Sampling is demand-driven, exactly like the daemon side: a weak per-host
//! view polls only while some consumer shows the host, and stops when every
//! window stopped showing it. The samples themselves live in a separate
//! run-scoped history keyed by `HostId`, so switching or closing a view can
//! neither destroy the current-run trend nor keep polling a host nobody
//! watches. The title-bar strip and the monitor's resource tiles read the
//! same history; unsampled stretches stay blank. The network series is
//! negotiated per peer (`host-network-metrics-v1`): an absent field means
//! unsupported on a peer without the name and a missing reading on one with
//! it — never a quiet zero.
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, Global, IntoElement, Render, WeakEntity, Window, div, fill,
    prelude::*, px, rems,
};
use lyrel_core::host::{
    Host as _, HostId,
    metrics::{HostMetrics, HostSample, MetricsState},
    remote::RemoteHost,
};

use crate::ui::app::LyrelApp;
use crate::ui::desktop_lookups::HostLinks;
use crate::ui::local_link::{LocalLink, LocalLinkStatus};
use crate::ui::remote_workspace::{MachineStatus, RemoteLinks};
use lyrel_ui_base::config::Config;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

/// How often the sampler asks its host for a reading while something shows it.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// The widest approved trend window (1 hour) sets how much history a run
/// keeps: older samples are dropped only once they can no longer appear.
const HISTORY_SPAN_MILLIS: u64 = 3_600_000;

/// Every approved window renders into the same number of display slots, so
/// long views aggregate samples per slot and keep the peak.
const BUCKET_COUNT: usize = 60;

/// Fixed strip widths handed to the chrome budget, matching the tiles built
/// in [`titlebar_metric`]: label + chart + value, or label + value.
const TITLEBAR_TILE_FULL_W: f32 = 132.;
const TITLEBAR_TILE_COMPACT_W: f32 = 76.;
const TITLEBAR_TILE_GAP: f32 = 12.;

/// Which title-bar strip the available width buys, if any: `Some(true)`
/// for full tiles with trends, `Some(false)` for compact readings, `None`
/// when even those would crowd the command groups, drag band or native
/// controls. The space those fixed pieces need is
/// [`crate::ui::window_chrome::titlebar_chrome_reserve`], not a second
/// guess at the same geometry, so the chart and the chrome cannot disagree.
pub(crate) fn titlebar_metrics_layout(
    viewport_width: f32,
    rem_size: f32,
    count: u8,
    fullscreen: bool,
) -> Option<bool> {
    if count == 0 {
        return None;
    }
    let available =
        viewport_width - crate::ui::window_chrome::titlebar_chrome_reserve(rem_size, fullscreen);
    let full = (TITLEBAR_TILE_FULL_W + TITLEBAR_TILE_GAP) * f32::from(count);
    let compact = (TITLEBAR_TILE_COMPACT_W + TITLEBAR_TILE_GAP) * f32::from(count);
    match available >= full {
        true => Some(true),
        false if available >= compact => Some(false),
        false => None,
    }
}

#[derive(Clone)]
pub(crate) enum MetricsSource {
    Local(Arc<lyrel_core::daemon::control::ControlClient>),
    Remote(Arc<RemoteHost>),
}

impl MetricsSource {
    pub(crate) fn get(host: HostId, cx: &mut App) -> Option<Self> {
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

    pub(crate) fn disk_usage(
        &self,
        dir: &str,
    ) -> std::io::Result<lyrel_core::host::disk::DiskUsage> {
        match self {
            Self::Local(client) => client.disk_usage(dir),
            Self::Remote(host) => host.disk_usage(std::path::Path::new(dir)),
        }
    }

    pub(crate) fn same_connection(&self, current: &Self) -> bool {
        match (self, current) {
            (Self::Local(old), Self::Local(current)) => Arc::ptr_eq(old, current),
            (Self::Remote(old), Self::Remote(current)) => Arc::ptr_eq(old, current),
            _ => false,
        }
    }

    fn response_state(
        &self,
        host: HostId,
        result: std::io::Result<HostMetrics>,
        cx: &mut App,
    ) -> State {
        let Some(current) = Self::get(host, cx).filter(Self::is_connected) else {
            return missing_source(host, cx);
        };
        match result {
            // A replacement control connection to this same daemon does not
            // invalidate a successful whole-host sample. The daemon instance,
            // not the Arc allocation, owns the measurement.
            Ok(snapshot) if snapshot.instance() == current.instance() => State::Ready(snapshot),
            Ok(_) => State::Waiting,
            // An old connection's error says nothing about its replacement,
            // even when both connections speak to the same daemon instance.
            Err(_) if !self.same_connection(&current) => State::Waiting,
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => State::Unsupported,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                missing_source(host, cx)
            }
            Err(error) => State::Failed(error.to_string()),
        }
    }

    pub(crate) fn instance(&self) -> &str {
        match self {
            Self::Local(client) => &client.hello().instance,
            Self::Remote(host) => &host.peer().instance,
        }
    }

    /// Whether the peer negotiated `host-network-metrics-v1`. An old or
    /// non-native peer never carries the network field, and the network tile
    /// must read that as unsupported rather than wait for it.
    fn supports_network(&self) -> bool {
        match self {
            Self::Local(client) => client
                .hello()
                .has_feature(lyrel_core::daemon::control::feature::HOST_NETWORK_METRICS),
            Self::Remote(host) => host
                .peer()
                .has_feature(lyrel_core::daemon::control::feature::HOST_NETWORK_METRICS),
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        match self {
            Self::Local(client) => client.is_connected(),
            Self::Remote(host) => host.client().is_connected(),
        }
    }
}

/// The demand side: one weak sampler view per host. While any consumer shows
/// the host, something holds the entity and its poll loop runs; when every
/// consumer lets go, the view drops and polling stops with it.
#[derive(Default)]
struct MetricsViews(HashMap<HostId, WeakEntity<HostMetricsView>>);
impl Global for MetricsViews {}

/// The retention side: run-scoped sample histories keyed by host, independent
/// of which view (if any) is alive. A view closing cannot destroy the trend.
#[derive(Default)]
struct MetricsHistories(HashMap<HostId, SampleHistory>);
impl Global for MetricsHistories {}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    Waiting,
    Connecting,
    Reconnecting,
    Ready(HostMetrics),
    Disconnected,
    Unsupported,
    Failed(String),
}

pub(crate) fn missing_source(host: HostId, cx: &App) -> State {
    if host.is_local() {
        return match LocalLink::status(cx) {
            LocalLinkStatus::Connecting => State::Connecting,
            LocalLinkStatus::Reconnecting => State::Reconnecting,
            // A supposedly connected source that is missing or just failed a
            // transport request must not pass its last sample off as current.
            LocalLinkStatus::Disconnected | LocalLinkStatus::Connected => State::Disconnected,
        };
    }
    match RemoteLinks::machine_status(cx, host) {
        Some(MachineStatus::Connecting) => State::Connecting,
        Some(MachineStatus::Reconnecting { .. }) => State::Reconnecting,
        _ => State::Disconnected,
    }
}

/// The approved trend windows. The selector and the charts speak this.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MetricsRange {
    #[default]
    TwoMinutes,
    QuarterHour,
    Hour,
}

impl MetricsRange {
    pub(crate) fn all() -> [Self; 3] {
        [Self::TwoMinutes, Self::QuarterHour, Self::Hour]
    }

    fn seconds(self) -> u64 {
        match self {
            Self::TwoMinutes => 120,
            Self::QuarterHour => 900,
            Self::Hour => 3600,
        }
    }

    fn label_key(self) -> L10nKey {
        match self {
            Self::TwoMinutes => L10nKey::MonitorRangeTwoMinutes,
            Self::QuarterHour => L10nKey::MonitorRangeQuarterHour,
            Self::Hour => L10nKey::MonitorRangeHour,
        }
    }
}

/// One aggregated display slot: the peak of every sample that fell into it,
/// so long windows compress without flattening spikes. `None` is a slot no
/// sample reached — an unsampled stretch stays blank rather than reading as
/// a quiet zero. GPU and network can be absent within populated slots.
#[derive(Clone, Copy)]
pub(crate) struct TrendBucket {
    pub(crate) cpu: f32,
    pub(crate) memory: f32,
    pub(crate) gpu: Option<f32>,
    pub(crate) network_rx: f32,
    pub(crate) network_tx: f32,
}

/// One kept reading, stamped with the GUI-side arrival time. The wire's own
/// clock is the remote's and is never compared against the GUI wall clock —
/// gaps and retention are honest local elapsed time.
#[derive(Clone)]
struct StampedSample {
    arrived_millis: u64,
    sample: HostSample,
}

/// The run-scoped sample store. Only successful readings are kept; a stretch
/// without readings is a gap the charts leave blank. Readings carry the
/// daemon instance they came from: a peer instance change drops the old
/// samples rather than letting the new daemon present its predecessor's.
#[derive(Default)]
struct SampleHistory {
    instance: Option<String>,
    arrivals: VecDeque<StampedSample>,
}

impl SampleHistory {
    fn push(&mut self, sample: HostSample, instance: &str) {
        // A zero total would divide by zero downstream and is not a real
        // memory reading, so it never enters the history.
        if sample.memory_total_bytes() == 0 {
            return;
        }
        if self.instance.as_deref() != Some(instance) {
            self.arrivals.clear();
            self.instance = Some(instance.to_owned());
        }
        self.arrivals.push_back(StampedSample {
            arrived_millis: now_millis(),
            sample,
        });
        while let Some(front) = self.arrivals.front() {
            let span = self.arrivals.back().map_or(0, |back| {
                back.arrived_millis.saturating_sub(front.arrived_millis)
            });
            if span > HISTORY_SPAN_MILLIS {
                self.arrivals.pop_front();
            } else {
                break;
            }
        }
    }

    fn latest(&self) -> Option<&StampedSample> {
        self.arrivals.back()
    }

    /// Peak-per-slot aggregation over `range`, ending at `anchor_millis`
    /// (the newest arrival while healthy, or now during a disconnect, so the
    /// blank tail after the last reading stays visible).
    fn buckets(&self, range: MetricsRange, anchor_millis: u64) -> Vec<Option<TrendBucket>> {
        let span = range.seconds() * 1000;
        let start = anchor_millis.saturating_sub(span);
        let slot = (span / BUCKET_COUNT as u64).max(1);
        let mut out: Vec<Option<TrendBucket>> = vec![None; BUCKET_COUNT];
        for arrival in &self.arrivals {
            let at = arrival.arrived_millis;
            if at < start || at > anchor_millis {
                continue;
            }
            // The freshest reading lands exactly on the anchor; clamp it into
            // the last slot rather than dropping it past the window edge.
            let ix = (((at - start) / slot).min(BUCKET_COUNT as u64 - 1)) as usize;
            let sample = &arrival.sample;
            let cpu = sample.cpu_percent().clamp(0., 100.);
            let memory = (sample.memory_percent() as f32).clamp(0., 100.);
            let gpu = sample.gpu_percent().map(|gpu| gpu.clamp(0., 100.));
            // A sample without a network reading leaves the slot's network
            // fields at negative infinity — a missing tick is a gap the
            // network chart skips, not a zero rate dragging on the peak.
            let (network_rx, network_tx) = sample
                .network()
                .map(|network| {
                    (
                        network.rx_bytes_per_sec() as f32,
                        network.tx_bytes_per_sec() as f32,
                    )
                })
                .unwrap_or((f32::NEG_INFINITY, f32::NEG_INFINITY));
            out[ix] = Some(match out[ix] {
                None => TrendBucket {
                    cpu,
                    memory,
                    gpu,
                    network_rx,
                    network_tx,
                },
                Some(prev) => TrendBucket {
                    cpu: prev.cpu.max(cpu),
                    memory: prev.memory.max(memory),
                    gpu: match (prev.gpu, gpu) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    },
                    network_rx: prev.network_rx.max(network_rx),
                    network_tx: prev.network_tx.max(network_tx),
                },
            });
        }
        out
    }
}

fn record_sample(host: HostId, sample: HostSample, instance: &str, cx: &mut App) {
    cx.default_global::<MetricsHistories>()
        .0
        .entry(host)
        .or_default()
        .push(sample, instance);
}

pub(crate) struct HostMetricsView {
    host: HostId,
    state: State,
}

impl HostMetricsView {
    /// The shared sampler for `host`, created on first demand and retained
    /// weakly: it lives only while some consumer shows the host. Title bar
    /// and monitor both enter here — one entity, one poll loop, one history
    /// per host.
    fn acquire(host: HostId, cx: &mut App) -> Entity<Self> {
        if let Some(view) = cx
            .default_global::<MetricsViews>()
            .0
            .get(&host)
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
                if this.upgrade().is_none() {
                    break;
                }
                let source = cx.update(|cx| MetricsSource::get(host, cx));
                let next = match source {
                    Some(source) => {
                        let query = source.clone();
                        let Some(result) =
                            lyrel_ui_base::host_ops::off_thread(move || query.sample()).await
                        else {
                            break;
                        };
                        cx.update(|cx| source.response_state(host, result, cx))
                    }
                    None => cx.update(|cx| missing_source(host, cx)),
                };
                if this
                    .update(cx, |view, cx| {
                        if let State::Ready(snapshot) = &next
                            && let MetricsState::Sample(sample) = snapshot.state()
                        {
                            record_sample(host, sample.clone(), snapshot.instance(), cx);
                        }
                        if view.state != next {
                            view.state = next;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(SAMPLE_INTERVAL).await;
            }
        })
        .detach();
        Self {
            host,
            state: if host.is_local() {
                if LocalLink::status(cx) == LocalLinkStatus::Connected {
                    State::Waiting
                } else {
                    missing_source(host, cx)
                }
            } else {
                State::Connecting
            },
        }
    }

    /// The render-time guard the status view always carried: a Ready sample
    /// is only trustworthy while its exact source is still connected for the
    /// same daemon instance. A lost or replaced route downgrades the reading
    /// before the next poll could, so a disconnect never displays stale
    /// numbers even for a frame.
    fn effective_state(&self, cx: &mut App) -> State {
        match &self.state {
            State::Ready(snapshot) => match MetricsSource::get(self.host, cx) {
                Some(source)
                    if source.is_connected() && source.instance() == snapshot.instance() =>
                {
                    self.state.clone()
                }
                Some(source) if source.is_connected() => State::Waiting,
                _ => missing_source(self.host, cx),
            },
            _ => self.state.clone(),
        }
    }

    /// Chart slots for `range`. Healthy samplers anchor at the newest
    /// arrival; a disconnected one anchors at now so the blank tail shows.
    fn trend(
        history: Option<&SampleHistory>,
        state: &State,
        range: MetricsRange,
    ) -> Vec<Option<TrendBucket>> {
        let anchor = match state {
            State::Ready(_) => history
                .and_then(SampleHistory::latest)
                .map_or_else(now_millis, |arrival| arrival.arrived_millis),
            _ => now_millis(),
        };
        history.map_or_else(|| vec![None; BUCKET_COUNT], |h| h.buckets(range, anchor))
    }
}

impl Render for HostMetricsView {
    // The view is a shared poll owner, never mounted: its paint is nothing.
    // The title-bar strip and the monitor tiles render from its state and
    // the retained history instead.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The freshest reading as presentation-ready text, plus the distinct state
/// word tiles show while there is nothing trustworthy to chart.
/// `network_supported` is the negotiated peer capability: it decides whether
/// an absent network field means an unsupported source ("—") or a sample
/// that will carry one shortly ("…").
fn readout(state: &State, network_supported: bool) -> MetricReadout {
    match state {
        State::Ready(snapshot) => match snapshot.state() {
            MetricsState::Sample(sample) => {
                let network = sample.network();
                MetricReadout {
                    cpu: Some(sample.cpu_percent()),
                    memory_used_bytes: Some(sample.memory_used_bytes()),
                    memory_total_bytes: Some(sample.memory_total_bytes()),
                    gpu: sample.gpu_percent(),
                    gpu_reason: sample.gpu_unavailable_reason().map(str::to_owned),
                    marker: None,
                    status: None,
                    detail: t_fmt(
                        L10nKey::HostMetricsSample,
                        &[
                            ("host", snapshot.hostname()),
                            ("used", &gb(sample.memory_used_bytes())),
                            ("total", &gb(sample.memory_total_bytes())),
                        ],
                    ),
                    network_supported,
                    network_rx_bytes_per_sec: network.map(|rate| rate.rx_bytes_per_sec()),
                    network_tx_bytes_per_sec: network.map(|rate| rate.tx_bytes_per_sec()),
                    // A missing network field on a supporting peer is the
                    // waiting family (fresh baseline, counter reset); on a
                    // peer without the capability it never arrives at all.
                    network_status: if network.is_some() {
                        None
                    } else if network_supported {
                        Some(t(L10nKey::HostMetricsWaiting))
                    } else {
                        Some(t(L10nKey::HostMetricsUnavailable))
                    },
                    network_detail: match network {
                        Some(rate) => t_fmt(
                            L10nKey::HostMetricsNetworkSample,
                            &[
                                ("host", snapshot.hostname()),
                                ("rx", &rate_text(rate.rx_bytes_per_sec())),
                                ("tx", &rate_text(rate.tx_bytes_per_sec())),
                            ],
                        ),
                        None if network_supported => t(L10nKey::HostMetricsWaiting).to_owned(),
                        None => t(L10nKey::HostMetricsUnavailable).to_owned(),
                    },
                }
            }
            MetricsState::Waiting => MetricReadout::pending(t(L10nKey::HostMetricsWaiting)),
            MetricsState::Unavailable(reason) => MetricReadout::down(
                t(L10nKey::HostMetricsUnavailable),
                t_fmt(L10nKey::HostMetricsFailed, &[("reason", reason)]),
            ),
        },
        State::Waiting => MetricReadout::pending(t(L10nKey::HostMetricsWaiting)),
        State::Connecting => MetricReadout::pending(t(L10nKey::HostMetricsConnecting)),
        State::Reconnecting => MetricReadout::pending(t(L10nKey::HostMetricsReconnecting)),
        State::Disconnected => MetricReadout::down(
            t(L10nKey::HostMetricsDisconnected),
            t(L10nKey::HostMetricsDisconnected).to_owned(),
        ),
        State::Unsupported => MetricReadout::down(
            t(L10nKey::HostMetricsUnavailable),
            t(L10nKey::HostMetricsUnavailable).to_owned(),
        ),
        State::Failed(reason) => MetricReadout::down(
            t(L10nKey::HostMetricsError),
            t_fmt(L10nKey::HostMetricsFailed, &[("reason", reason)]),
        ),
    }
}

struct MetricReadout {
    cpu: Option<f32>,
    memory_used_bytes: Option<u64>,
    memory_total_bytes: Option<u64>,
    /// The busiest-GPU-engine percentage; `None` while the sample carries
    /// no GPU reading (unavailable, or the peer predates GPU sampling).
    gpu: Option<f32>,
    /// The sampler's own reason for that miss, shown as the GPU slot's
    /// state word so the tile explains itself instead of bare `—`.
    gpu_reason: Option<String>,
    /// `Some(Pending)` is the "…" family (waiting/connecting), `Some(Down)`
    /// the "—" family (disconnected/unsupported/failed); `None` while a real
    /// sample is on screen.
    marker: Option<MetricMarker>,
    /// The short state word a tile shows beside its label. Shared by the
    /// whole readout; the GPU slot prefers [`Self::gpu_reason`].
    status: Option<gpui_kit::SharedString>,
    /// The hover detail: host, used and total memory.
    detail: String,
    /// Whether the peer negotiated `host-network-metrics-v1`.
    network_supported: bool,
    /// Off-host receive/transmit rates of the freshest sample; both or
    /// neither. `None` is a missing reading, never a quiet interface.
    network_rx_bytes_per_sec: Option<f64>,
    network_tx_bytes_per_sec: Option<f64>,
    /// The state word the network tile shows beside its label: absent while
    /// a real reading is on screen.
    network_status: Option<&'static str>,
    /// The network tile's hover detail: host, receive and transmit rates.
    network_detail: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetricMarker {
    Pending,
    Down,
}

impl MetricReadout {
    fn pending(status: &'static str) -> Self {
        Self {
            cpu: None,
            memory_used_bytes: None,
            memory_total_bytes: None,
            gpu: None,
            gpu_reason: None,
            marker: Some(MetricMarker::Pending),
            status: Some(status.into()),
            detail: status.to_owned(),
            network_supported: false,
            network_rx_bytes_per_sec: None,
            network_tx_bytes_per_sec: None,
            network_status: Some(status),
            network_detail: status.to_owned(),
        }
    }

    fn down(status: &'static str, detail: String) -> Self {
        Self {
            cpu: None,
            memory_used_bytes: None,
            memory_total_bytes: None,
            gpu: None,
            gpu_reason: None,
            marker: Some(MetricMarker::Down),
            status: Some(status.into()),
            detail,
            network_supported: false,
            network_rx_bytes_per_sec: None,
            network_tx_bytes_per_sec: None,
            network_status: Some(status),
            network_detail: status.to_owned(),
        }
    }

    fn ready(&self) -> bool {
        self.cpu.is_some()
    }

    fn network_ready(&self) -> bool {
        self.network_rx_bytes_per_sec.is_some() && self.network_tx_bytes_per_sec.is_some()
    }

    fn marker_text(&self) -> &'static str {
        match self.marker {
            Some(MetricMarker::Pending) => "…",
            _ => "—",
        }
    }

    fn cpu_text(&self) -> String {
        match self.cpu {
            Some(cpu) => format!("{:.0}%", cpu),
            None => self.marker_text().into(),
        }
    }

    /// The title bar's RAM reading carries its unit beside CPU's and GPU's
    /// percentages: `13.1 GB`.
    fn memory_text(&self) -> String {
        match self.memory_used_bytes {
            Some(used) => format!("{} GB", gb(used)),
            None => self.marker_text().into(),
        }
    }

    /// The monitor tile's used/total pair, e.g. `13.1 GB / 32 GB`.
    fn memory_pair_text(&self) -> String {
        match self.memory_total_bytes {
            Some(total) => format!("{} / {} GB", self.memory_text(), gb(total)),
            None => self.memory_text(),
        }
    }

    fn gpu_text(&self) -> String {
        match self.gpu {
            Some(gpu) => format!("{:.0}%", gpu),
            None => self.marker_text().into(),
        }
    }

    /// The state word the GPU slot shows beside its label: the sampler's
    /// reason while a sample is on screen without a GPU reading, otherwise
    /// the shared pending/down word.
    fn gpu_status(&self) -> Option<gpui_kit::SharedString> {
        if self.gpu.is_some() {
            return None;
        }
        self.gpu_reason
            .as_deref()
            .map(Into::into)
            .or_else(|| self.status.clone())
    }

    /// The state word a monitor tile shows beside its label for `series`:
    /// the GPU slot prefers its unavailable reason over the shared word.
    fn status_for(&self, series: TrendSeries) -> Option<gpui_kit::SharedString> {
        match series {
            TrendSeries::Gpu => self.gpu_status(),
            _ => self.status.clone(),
        }
    }
    /// `↓ 2.1 MB/s`, or the marker family while nothing trustworthy is on
    /// screen: "…" while waiting on a supporting peer, "—" where the peer
    /// will never carry one (or the whole sampler is down).
    fn network_rx_text(&self) -> String {
        self.network_text(self.network_rx_bytes_per_sec, "↓")
    }

    /// The transmit half of [`Self::network_rx_text`], drawn dashed in the chart.
    fn network_tx_text(&self) -> String {
        self.network_text(self.network_tx_bytes_per_sec, "↑")
    }

    fn network_text(&self, rate: Option<f64>, arrow: &str) -> String {
        if let Some(rate) = rate {
            return format!("{arrow} {}", rate_text(rate));
        }
        // Without a rate the marker family speaks: the whole-sampler states
        // carry their own marker; a ready sample either waits on a
        // supporting peer ("…") or will never carry a reading ("—").
        match (self.marker, self.network_supported) {
            (Some(MetricMarker::Down), _) => format!("{arrow} —"),
            (Some(MetricMarker::Pending), _) => format!("{arrow} …"),
            (None, true) => format!("{arrow} …"),
            (None, false) => format!("{arrow} —"),
        }
    }
}

/// Bytes per second in the SI units link rates use (1 KB/s = 1000 B/s), one
/// decimal trimmed the way [`gb`] trims it: `512 B/s`, `240 KB/s`, `2.1 MB/s`.
pub(super) fn rate_text(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_sec.max(0.);
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1000.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[0])
    } else {
        format!(
            "{} {}",
            format!("{value:.1}").trim_end_matches(".0"),
            UNITS[unit]
        )
    }
}

fn gb(bytes: u64) -> String {
    let gib = bytes as f64 / 1_073_741_824.0;
    let text = format!("{gib:.1}");
    text.trim_end_matches(".0").to_owned()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// Which series a chart draws. Later resource tickets add their fields to
/// [`TrendBucket`] and their series here; the network tile instead paints its
/// two series together in [`network_trend_chart`], so it needs no variant.
#[derive(Clone, Copy)]
enum TrendSeries {
    Cpu,
    Memory,
    Gpu,
}

impl TrendSeries {
    /// The series value out of a reached slot; `None` (GPU only, for now)
    /// paints a gap in that slot instead of a bar or step.
    fn get(self) -> fn(&TrendBucket) -> Option<f32> {
        match self {
            Self::Cpu => |bucket: &TrendBucket| Some(bucket.cpu),
            Self::Memory => |bucket: &TrendBucket| Some(bucket.memory),
            Self::Gpu => |bucket: &TrendBucket| bucket.gpu,
        }
    }

    /// Bars for utilization rates, a step line for quantities.
    fn line(self) -> bool {
        match self {
            Self::Cpu => false,
            Self::Memory => true,
            Self::Gpu => false,
        }
    }
}

/// A sparkline drawn straight into a canvas: one quad per bar or step
/// segment, nothing for a blank slot, so gaps read as gaps. `scale` is the
/// value that maps to the full height — 100 for utilization percentages, the
/// window peak for throughput series.
fn trend_chart(
    id: impl Into<gpui_kit::ElementId>,
    buckets: Vec<Option<TrendBucket>>,
    series: TrendSeries,
    color: gpui_kit::Hsla,
    scale: f32,
    width: f32,
    height: f32,
) -> AnyElement {
    let value = series.get();
    let line = series.line();
    div()
        .id(id)
        .test_support()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .child(gpui_kit::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let width_px = bounds.size.width.as_f32().min(width);
                let height_px = bounds.size.height.as_f32().min(height);
                if width_px <= 0. || height_px <= 0. {
                    return;
                }
                let slot = width_px / BUCKET_COUNT as f32;
                let x = |ix: usize| bounds.origin.x + px((ix as f32 + 0.5) * slot);
                let y = |v: f32| {
                    bounds.origin.y + px(height_px - (v.clamp(0., scale) / scale) * height_px)
                };
                if line {
                    let stroke = 1.5_f32.min(slot * 0.4).max(1.);
                    let mut prev: Option<(usize, f32)> = None;
                    for (ix, bucket) in buckets.iter().enumerate() {
                        // A blank slot, or a slot whose samples carry no
                        // value for this series, breaks the run — a gap
                        // stays a gap either way.
                        let Some(bucket) = bucket else {
                            prev = None;
                            continue;
                        };
                        let Some(v) = value(bucket) else {
                            prev = None;
                            continue;
                        };
                        let point = (ix, v);
                        if let Some((pix, pv)) = prev {
                            let (x0, x1) = (x(pix), x(ix));
                            let (y0, y1) = (y(pv), y(v));
                            let run = (x1 - x0).max(px(stroke));
                            window.paint_quad(fill(
                                gpui_kit::Bounds {
                                    origin: gpui_kit::point(x0, y0 - px(stroke / 2.)),
                                    size: gpui_kit::size(run, px(stroke)),
                                },
                                color,
                            ));
                            let rise = y1 - y0;
                            if rise.abs() > px(stroke) {
                                window.paint_quad(fill(
                                    gpui_kit::Bounds {
                                        origin: gpui_kit::point(x1 - px(stroke / 2.), y0.min(y1)),
                                        size: gpui_kit::size(px(stroke), rise.abs()),
                                    },
                                    color,
                                ));
                            }
                        }
                        prev = Some(point);
                    }
                } else {
                    let bar_w = (slot * 0.6).max(1.);
                    for (ix, bucket) in buckets.iter().enumerate() {
                        let Some(bucket) = bucket else { continue };
                        let Some(v) = value(bucket) else { continue };
                        let bar_h = ((v.clamp(0., scale) / scale) * height_px).max(1.);
                        window.paint_quad(fill(
                            gpui_kit::Bounds {
                                origin: gpui_kit::point(
                                    bounds.origin.x + px(ix as f32 * slot + (slot - bar_w) / 2.),
                                    bounds.origin.y + px(height_px - bar_h),
                                ),
                                size: gpui_kit::size(px(bar_w), px(bar_h)),
                            },
                            color,
                        ));
                    }
                }
            },
        ))
        .into_any_element()
}

/// One title-bar metric: label, short trend, current value. Clicking it
/// opens the Activity Monitor on the same samples.
fn titlebar_metric(
    id: &'static str,
    label: &'static str,
    readout: &MetricReadout,
    buckets: Vec<Option<TrendBucket>>,
    series: TrendSeries,
    color: gpui_kit::Hsla,
    chart: bool,
    cx: &mut Context<LyrelApp>,
) -> AnyElement {
    let detail = readout.detail.clone();
    let value = match series {
        TrendSeries::Cpu => readout.cpu_text(),
        TrendSeries::Memory => readout.memory_text(),
        TrendSeries::Gpu => readout.gpu_text(),
    };
    h_flex()
        .id(id)
        .aria_label(gpui_kit::SharedString::from(detail.clone()))
        .test_support()
        .w(px(if chart {
            TITLEBAR_TILE_FULL_W
        } else {
            TITLEBAR_TILE_COMPACT_W
        }))
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .occlude()
        .on_click(cx.listener(move |this, _, window, cx| {
            this.open_activity_monitor(window, cx);
        }))
        .tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .when(chart && readout.ready(), |metric| {
            metric.child(trend_chart(
                format!("{id}-chart"),
                buckets,
                series,
                color,
                100.,
                40.,
                14.,
            ))
        })
        .child(
            div()
                .id(format!("{id}-value"))
                .aria_label(gpui_kit::SharedString::from(value.clone()))
                .test_support()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(gpui_kit::SharedString::from(value)),
        )
        .into_any_element()
}

/// One resource tile in the monitor: label, the shared reading, and the
/// range's trend underneath. A pending or failed sampler replaces the value
/// with its distinct state word and leaves the chart blank. Each series
/// derives its own state word — the GPU tile speaks its unavailable reason.
fn monitor_tile(
    id: &'static str,
    label: &'static str,
    readout: &MetricReadout,
    buckets: Vec<Option<TrendBucket>>,
    series: TrendSeries,
    color: gpui_kit::Hsla,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let value = match series {
        TrendSeries::Cpu => readout.cpu_text(),
        TrendSeries::Memory => readout.memory_pair_text(),
        TrendSeries::Gpu => readout.gpu_text(),
    };
    v_flex()
        .id(id)
        .aria_label(gpui_kit::SharedString::from(readout.detail.clone()))
        .test_support()
        .flex_1()
        .min_w_0()
        .p(px(8.))
        .gap(px(6.))
        .rounded(px(7.))
        .bg(theme.muted.opacity(0.18))
        .child(
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(rems(11. / 16.))
                        .text_color(color)
                        .child(label),
                )
                .when_some(readout.status_for(series), |row, status| {
                    row.child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.)
                            .text_ellipsis()
                            .text_color(theme.muted_foreground)
                            .child(status),
                    )
                }),
        )
        .child(
            div()
                .id(format!("{id}-value"))
                .aria_label(gpui_kit::SharedString::from(value.clone()))
                .test_support()
                .text_size(rems(17. / 16.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(gpui_kit::SharedString::from(value)),
        )
        .when(readout.ready(), |tile| {
            tile.child(trend_chart(
                format!("{id}-chart"),
                buckets,
                series,
                color,
                100.,
                108.,
                24.,
            ))
        })
        .into_any_element()
}

/// The network tile's two series in one canvas, per the approved figure:
/// receive as a solid step line and transmit as a dashed step line, both
/// scaled to the shared window peak so heights stay comparable. A blank slot
/// or a slot no network reading reached breaks both lines; a genuine zero
/// flattens them at the floor — idle and missing never look the same.
fn network_trend_chart(
    id: impl Into<gpui_kit::ElementId>,
    buckets: Vec<Option<TrendBucket>>,
    rx_color: gpui_kit::Hsla,
    tx_color: gpui_kit::Hsla,
    width: f32,
    height: f32,
) -> AnyElement {
    // A slot that never saw a network reading carries negative infinity; it
    // never wins the peak and the painters skip it, so unsampled time stays
    // blank rather than dragging the scale.
    let peak = buckets
        .iter()
        .flatten()
        .flat_map(|bucket| [bucket.network_rx, bucket.network_tx])
        .filter(|value| value.is_finite())
        .fold(0_f32, f32::max)
        .max(1.);
    div()
        .id(id)
        .test_support()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .child(gpui_kit::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let width_px = bounds.size.width.as_f32().min(width);
                let height_px = bounds.size.height.as_f32().min(height);
                if width_px <= 0. || height_px <= 0. {
                    return;
                }
                let slot = width_px / BUCKET_COUNT as f32;
                let x = |ix: usize| bounds.origin.x + px((ix as f32 + 0.5) * slot);
                let y = |v: f32| {
                    bounds.origin.y + px(height_px - (v.clamp(0., peak) / peak) * height_px)
                };
                let stroke = 1.5_f32.min(slot * 0.4).max(1.);

                // One shared geometry for both step series; the painter is
                // what differs (solid vs dashed).
                struct StepSeries {
                    color: gpui_kit::Hsla,
                    dashed: bool,
                }
                for series in [
                    StepSeries {
                        color: rx_color,
                        dashed: false,
                    },
                    StepSeries {
                        color: tx_color,
                        dashed: true,
                    },
                ] {
                    let value = |bucket: &TrendBucket| {
                        if series.dashed {
                            bucket.network_tx
                        } else {
                            bucket.network_rx
                        }
                    };
                    let mut prev: Option<(usize, f32)> = None;
                    for (ix, bucket) in buckets.iter().enumerate() {
                        let Some(bucket) = bucket else {
                            prev = None;
                            continue;
                        };
                        let v = value(bucket);
                        if !v.is_finite() {
                            prev = None;
                            continue;
                        }
                        if let Some((pix, pv)) = prev {
                            let (x0, x1) = (x(pix), x(ix));
                            let (y0, y1) = (y(pv), y(v));
                            if series.dashed {
                                paint_dashed_h(window, x0, x1, y0, stroke, series.color);
                            } else {
                                let run = (x1 - x0).max(px(stroke));
                                window.paint_quad(fill(
                                    gpui_kit::Bounds {
                                        origin: gpui_kit::point(x0, y0 - px(stroke / 2.)),
                                        size: gpui_kit::size(run, px(stroke)),
                                    },
                                    series.color,
                                ));
                            }
                            let rise = y1 - y0;
                            if rise.abs() > px(stroke) {
                                window.paint_quad(fill(
                                    gpui_kit::Bounds {
                                        origin: gpui_kit::point(x1 - px(stroke / 2.), y0.min(y1)),
                                        size: gpui_kit::size(px(stroke), rise.abs()),
                                    },
                                    series.color,
                                ));
                            }
                        }
                        prev = Some((ix, v));
                    }
                }
            },
        ))
        .into_any_element()
}

/// Paint one horizontal run of the dashed transmit series: `DASH` on, `GAP`
/// off, measured along the run, clipping the tail. Runs shorter than a gap
/// vanish inside it exactly like a real dashed stroke; the vertical risers
/// stay solid so the step shape survives at small widths.
fn paint_dashed_h(
    window: &mut gpui_kit::Window,
    x0: gpui_kit::Pixels,
    x1: gpui_kit::Pixels,
    y: gpui_kit::Pixels,
    stroke: f32,
    color: gpui_kit::Hsla,
) {
    const DASH: f32 = 3.;
    const GAP: f32 = 2.;
    let mut cursor = x0;
    while cursor < x1 {
        let dash_end = cursor + px(DASH);
        let end = if dash_end < x1 { dash_end } else { x1 };
        window.paint_quad(fill(
            gpui_kit::Bounds {
                origin: gpui_kit::point(cursor, y - px(stroke / 2.)),
                size: gpui_kit::size(end - cursor, px(stroke)),
            },
            color,
        ));
        cursor = end + px(GAP);
    }
}

/// The Activity Monitor's network tile (#375): one tile grouping the host's
/// receive and send series per the approved figure — a label with the
/// distinct state word while nothing trustworthy is on screen, one row with
/// the ↓ receive and ↑ send rates, and both trends overlaid underneath.
/// Unsupported peers and missing readings keep their distinct markers
/// instead of reading as a quiet zero.
fn monitor_network_tile(
    id: &'static str,
    label: &'static str,
    readout: &MetricReadout,
    buckets: Vec<Option<TrendBucket>>,
    rx_color: gpui_kit::Hsla,
    tx_color: gpui_kit::Hsla,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .aria_label(gpui_kit::SharedString::from(readout.network_detail.clone()))
        .test_support()
        .flex_1()
        .min_w_0()
        .p(px(8.))
        .gap(px(6.))
        .rounded(px(7.))
        .bg(theme.muted.opacity(0.18))
        .child(
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(rems(11. / 16.))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .when_some(readout.network_status, |row, status| {
                    row.child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .text_color(theme.muted_foreground)
                            .child(status),
                    )
                }),
        )
        .child(
            h_flex()
                .items_end()
                .justify_between()
                .child(
                    div()
                        .id(format!("{id}-rx"))
                        .aria_label(gpui_kit::SharedString::from(readout.network_rx_text()))
                        .test_support()
                        .text_size(rems(13. / 16.))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(gpui_kit::SharedString::from(readout.network_rx_text())),
                )
                .child(
                    div()
                        .id(format!("{id}-tx"))
                        .aria_label(gpui_kit::SharedString::from(readout.network_tx_text()))
                        .test_support()
                        .text_size(rems(11. / 16.))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(gpui_kit::SharedString::from(readout.network_tx_text())),
                ),
        )
        .when(readout.network_ready(), |tile| {
            tile.child(network_trend_chart(
                format!("{id}-chart"),
                buckets,
                rx_color,
                tx_color,
                108.,
                24.,
            ))
        })
        .into_any_element()
}

impl LyrelApp {
    /// The host whose metrics this window shows: the context pane's host,
    /// falling back to the workspace host before any pane exists.
    pub(crate) fn metric_host(&self, window: &Window, cx: &App) -> HostId {
        self.context_pane(window, cx)
            .map_or_else(|| self.spawn_host(cx), |leaf| leaf.read(cx).host_id())
    }

    /// Which title-bar strip this frame can afford, if any: the same
    /// predicate drives demand (sampling) and paint, so a hidden strip never
    /// keeps sampling and a shown one never disagrees about charts.
    fn titlebar_metrics_state(&self, window: &Window, cx: &Context<Self>) -> Option<bool> {
        let toolbar = cx.global::<Config>().toolbar;
        if !toolbar.cpu && !toolbar.ram && !toolbar.gpu {
            return None;
        }
        if self.status_document_context(window, cx).is_some() {
            return None;
        }
        let count = u8::from(toolbar.cpu) + u8::from(toolbar.ram) + u8::from(toolbar.gpu);
        titlebar_metrics_layout(
            window.viewport_size().width.as_f32(),
            window.rem_size().as_f32(),
            count,
            window.is_fullscreen(),
        )
    }

    /// Whether the title-bar strip shows at all this frame. The strip and
    /// the sync share this one predicate, so demand and paint never disagree.
    fn titlebar_metrics_visible(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.titlebar_metrics_state(window, cx).is_some()
    }

    /// Keep exactly one sampler alive for the host some consumer is showing,
    /// and release it when none is. The title bar counts while its strip is
    /// visible; the monitor counts while its tool is open on a live pane.
    /// Releasing the view stops its poll loop — the run-scoped history in
    /// `MetricsHistories` is what survives being offscreen.
    pub(crate) fn sync_host_metrics(&mut self, window: &Window, cx: &mut Context<Self>) {
        let host = self.metric_host(window, cx);
        let monitor_shown = self.right_panel_visible
            && matches!(
                self.right_panel_tab,
                lyrel_core::core::config::RightPanelTab::Monitor
            )
            && self.right_panel.procs_pane.is_some();
        let want = self.titlebar_metrics_visible(window, cx) || monitor_shown;
        let current = self.metric_view.take();
        self.metric_view = match (want, current) {
            (true, Some(view)) if view.read(cx).host == host => Some(view),
            (true, _) => Some(HostMetricsView::acquire(host, cx)),
            (false, _) => None,
        };
    }

    pub(crate) fn open_activity_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_right_panel_tab(lyrel_core::core::config::RightPanelTab::Monitor, window, cx);
    }

    /// The shared readout and trend for `host`, from the one live sampler
    /// and the retained history. The network series rides the same sample;
    /// its unsupported-vs-waiting distinction comes from the peer's
    /// negotiated capability, not from the sample itself.
    fn metric_presentation(
        &self,
        host: HostId,
        range: MetricsRange,
        cx: &mut App,
    ) -> (MetricReadout, Vec<Option<TrendBucket>>) {
        let state = match self
            .metric_view
            .as_ref()
            .filter(|view| view.read(cx).host == host)
        {
            Some(view) => view.update(cx, |view, cx| view.effective_state(cx)),
            None => missing_source(host, cx),
        };
        let network_supported = MetricsSource::get(host, cx).is_some_and(|s| s.supports_network());
        let history = cx.default_global::<MetricsHistories>().0.get(&host);
        (
            readout(&state, network_supported),
            HostMetricsView::trend(history, &state, range),
        )
    }

    /// The title-bar strip, or `None` where it must not appear. Narrow
    /// windows drop the charts first and the whole strip last; the readings
    /// themselves never shrink the native controls or the drag area.
    pub(crate) fn render_titlebar_metrics(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, f32)> {
        let chart = self.titlebar_metrics_state(window, cx)?;
        let toolbar = cx.global::<Config>().toolbar;
        let host = self.metric_host(window, cx);
        let (readout, mut buckets) = self.metric_presentation(host, MetricsRange::TwoMinutes, cx);
        let theme = cx.theme();
        let cpu_color = theme.accent;
        let memory_color = theme.success;
        let gpu_color = theme.warning;
        let gap = px(TITLEBAR_TILE_GAP);
        let strip = h_flex()
            .id("titlebar-metrics")
            .test_support()
            .flex_none()
            .items_center()
            .gap(gap)
            .pr(gap)
            .when(toolbar.cpu, |strip| {
                strip.child(titlebar_metric(
                    "titlebar-metric-cpu",
                    "CPU",
                    &readout,
                    if toolbar.ram || toolbar.gpu {
                        buckets.clone()
                    } else {
                        std::mem::take(&mut buckets)
                    },
                    TrendSeries::Cpu,
                    cpu_color,
                    chart,
                    cx,
                ))
            })
            .when(toolbar.ram, |strip| {
                strip.child(titlebar_metric(
                    "titlebar-metric-ram",
                    t(L10nKey::HostMetricsRam),
                    &readout,
                    if toolbar.gpu {
                        buckets.clone()
                    } else {
                        std::mem::take(&mut buckets)
                    },
                    TrendSeries::Memory,
                    memory_color,
                    chart,
                    cx,
                ))
            })
            .when(toolbar.gpu, |strip| {
                strip.child(titlebar_metric(
                    "titlebar-metric-gpu",
                    "GPU",
                    &readout,
                    buckets,
                    TrendSeries::Gpu,
                    gpu_color,
                    chart,
                    cx,
                ))
            })
            .into_any_element();
        let tile_w = if chart {
            TITLEBAR_TILE_FULL_W
        } else {
            TITLEBAR_TILE_COMPACT_W
        };
        let count = u8::from(toolbar.cpu) + u8::from(toolbar.ram) + u8::from(toolbar.gpu);
        Some((strip, (tile_w + TITLEBAR_TILE_GAP) * f32::from(count)))
    }

    /// The Activity Monitor's resource section: the shared trend-range
    /// selector and one tile per resource slot. Network and disk slot in
    /// here as their tickets land; the sampler, history and chart code
    /// above is already theirs to extend.
    pub(crate) fn render_monitor_resources(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host = self.metric_host(window, cx);
        let range = self.monitor.range;
        let (readout, buckets) = self.metric_presentation(host, range, cx);
        let theme = cx.theme();
        let cpu_color = theme.accent;
        let memory_color = theme.success;
        let gpu_color = theme.warning;
        let muted = theme.muted_foreground;

        let selector = h_flex()
            .id("monitor-range-selector")
            .test_support()
            .items_center()
            .gap(px(2.))
            .children(MetricsRange::all().into_iter().map(|option| {
                let selected = option == range;
                let label = t(option.label_key());
                div()
                    .id(format!("monitor-range-{}", option.seconds()))
                    .aria_label(gpui_kit::SharedString::from(label))
                    .test_support()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_size(rems(11. / 16.))
                    .when(selected, |button| button.bg(theme.muted.opacity(0.5)))
                    .text_color(match selected {
                        true => theme.foreground,
                        false => muted,
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.monitor.range = option;
                        cx.notify();
                    }))
                    .child(label)
            }))
            .into_any_element();

        let heading = h_flex()
            .id("monitor-resources-heading")
            .test_support()
            .px(px(crate::ui::right_panel::TEXT_INSET))
            .pb(px(6.))
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(crate::ui::right_panel::META))
                    .text_color(muted)
                    .child(t(L10nKey::MonitorResourcesTitle)),
            )
            .child(selector)
            .into_any_element();

        let tiles = h_flex()
            .id("monitor-resource-tiles")
            .test_support()
            .px(px(crate::ui::window_chrome::CONTENT_INSET))
            .gap(px(8.))
            .child(monitor_tile(
                "monitor-tile-cpu",
                t(L10nKey::MonitorColCpu),
                &readout,
                buckets.clone(),
                TrendSeries::Cpu,
                cpu_color,
                cx,
            ))
            .child(monitor_tile(
                "monitor-tile-ram",
                t(L10nKey::MonitorColMemory),
                &readout,
                buckets.clone(),
                TrendSeries::Memory,
                memory_color,
                cx,
            ))
            .child(monitor_tile(
                "monitor-tile-gpu",
                t(L10nKey::MonitorColGpu),
                &readout,
                buckets.clone(),
                TrendSeries::Gpu,
                gpu_color,
                cx,
            ))
            .into_any_element();

        // The approved figure puts the network tile in the second row, beside
        // the disk tile. The network series ride the same samples and range
        // selector; receive draws solid and transmit dashed per the figure's
        // palette (the accent family for receive, the warning family for
        // transmit).
        let network_tiles = h_flex()
            .id("monitor-resource-tiles-secondary")
            .test_support()
            .px(px(crate::ui::window_chrome::CONTENT_INSET))
            .gap(px(8.))
            .child(monitor_network_tile(
                "monitor-tile-network",
                t(L10nKey::MonitorColNetwork),
                &readout,
                buckets,
                theme.accent,
                theme.warning,
                cx,
            ))
            .child(self.render_disk_tile(window, cx))
            .into_any_element();

        v_flex()
            .id("monitor-resources")
            .test_support()
            .flex_none()
            .child(heading)
            .child(tiles)
            .child(network_tiles)
            .child(self.render_lyrel_usage(cx))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "host_metrics_tests.rs"]
pub(crate) mod tests;
