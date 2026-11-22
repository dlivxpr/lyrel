//! The disk tile's scan owner (#376): one bounded scan per (host, directory),
//! independent of the live metrics cadence.
//!
//! The 1-second metrics loop refreshes readings a tile can chart. A directory
//! scan is a heavyweight walk that runs once per (host, directory) change,
//! never on the metrics timer: the tile shows the latest completed scan for
//! the directory it is bound to, marked as in flight while its scan runs, and
//! a scan that answers after its directory was replaced is dropped, never
//! displayed. Only the route-missing states retry — when the link to the
//! host comes back, the next sync starts one fresh scan; a failed or
//! unsupported scan stays put until the directory or host changes.
use std::collections::HashMap;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, Global, IntoElement, Render, WeakEntity, Window, div,
    prelude::*, px, rems,
};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::host::{
    HostId,
    disk::{DiskState as WireDiskState, DiskUsage},
};

use crate::ui::app::LyrelApp;
use crate::ui::desktop_lookups;
use crate::ui::home::display_path;
use crate::ui::host_metrics::{MetricsSource, State as MetricState, missing_source};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

/// What one (host, directory) pair's scan is doing. Unlike the metrics
/// sampler there is no `Waiting`-for-sample state: the scan runs on request,
/// so the in-flight state is [`DiskViewState::Scanning`] and a completed scan
/// carries the stamped wire answer it will display until rescanned.
#[derive(Clone, Debug, PartialEq)]
enum DiskViewState {
    /// The scan request is in flight for this view's exact directory.
    Scanning,
    /// The latest completed scan for this view's directory; the wire stamp
    /// fences it against a replaced daemon instance.
    Ready(DiskUsage),
    Disconnected,
    Connecting,
    Reconnecting,
    /// The transport or the peer refuses directory scans.
    Unsupported,
    /// The scan ran and failed; the reason says why (missing directory,
    /// unreadable root, platform without a volume table...).
    Failed(String),
}

/// The demand side: one scan view per (host, directory), held weakly. While
/// the monitor shows the pair, something holds the entity and its single
/// scan stays live; when the pair changes or the monitor hides, the view
/// drops and nothing keeps scanning it.
#[derive(Default)]
struct DiskViews(HashMap<(HostId, String), WeakEntity<HostDiskView>>);
impl Global for DiskViews {}

pub(crate) struct HostDiskView {
    host: HostId,
    dir: String,
    state: DiskViewState,
}

impl HostDiskView {
    /// The scan view for one exact pair, created on first demand. Retrying a
    /// route-missing state is the caller's job (see [`LyrelApp::sync_host_disk`]):
    /// this only ever starts one scan per entity.
    fn acquire(host: HostId, dir: String, cx: &mut App) -> Entity<Self> {
        if let Some(view) = cx
            .default_global::<DiskViews>()
            .0
            .get(&(host, dir.clone()))
            .and_then(WeakEntity::upgrade)
        {
            return view;
        }
        let view = cx.new(|cx| Self::new(host, dir.clone(), cx));
        let views = &mut cx.default_global::<DiskViews>().0;
        views.retain(|_, view| view.upgrade().is_some());
        views.insert((host, dir), view.downgrade());
        view
    }

    fn new(host: HostId, dir: String, cx: &mut Context<Self>) -> Self {
        let task_host = host;
        let task_dir = dir.clone();
        cx.spawn(async move |this, cx| {
            let state = match cx.update(|cx| MetricsSource::get(task_host, cx)) {
                Some(source) => {
                    let query = source.clone();
                    let requested = task_dir.clone();
                    let Some(result) =
                        lyrel_ui_base::host_ops::off_thread(move || query.disk_usage(&requested))
                            .await
                    else {
                        return;
                    };
                    cx.update(|cx| disk_response_state(&source, task_host, &task_dir, result, cx))
                }
                None => cx.update(|cx| missing_disk_source(task_host, cx)),
            };
            if this
                .update(cx, |view, cx| {
                    view.state = state;
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            // One scan per view: no timer, no poll. The next scan of this
            // directory belongs to the next view.
        })
        .detach();
        Self {
            host,
            dir,
            state: DiskViewState::Scanning,
        }
    }

    /// The render-time guard, same contract as the metrics view's: a
    /// completed scan is only trustworthy while its exact source is still
    /// connected for the same daemon instance.
    fn effective_state(&self, cx: &mut App) -> DiskViewState {
        match &self.state {
            DiskViewState::Ready(usage) => match MetricsSource::get(self.host, cx) {
                Some(source) if source.is_connected() && source.instance() == usage.instance() => {
                    self.state.clone()
                }
                Some(source) if source.is_connected() => DiskViewState::Scanning,
                _ => missing_disk_source(self.host, cx),
            },
            _ => self.state.clone(),
        }
    }
}

impl Render for HostDiskView {
    // The view is a scan owner, never mounted: the resource tiles render
    // from its state instead.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Map a route-missing metrics state onto the disk view's states; the two
/// speak the same link lifecycle.
fn missing_disk_source(host: HostId, cx: &App) -> DiskViewState {
    match missing_source(host, cx) {
        MetricState::Connecting => DiskViewState::Connecting,
        MetricState::Reconnecting => DiskViewState::Reconnecting,
        _ => DiskViewState::Disconnected,
    }
}

/// Where a scan answer lands. A stale directory (the view's directory moved
/// on), a replaced daemon instance, or an old connection's error never
/// replace what the tile shows: the view stays in flight for its own
/// directory and only an answer for exactly that pair completes it.
fn disk_response_state(
    source: &MetricsSource,
    host: HostId,
    dir: &str,
    result: std::io::Result<DiskUsage>,
    cx: &mut App,
) -> DiskViewState {
    let Some(current) = MetricsSource::get(host, cx).filter(MetricsSource::is_connected) else {
        return missing_disk_source(host, cx);
    };
    match result {
        Ok(usage) if usage.instance() == current.instance() && usage.directory() == dir => {
            match usage.state() {
                WireDiskState::Scanned(_) => DiskViewState::Ready(usage),
                // The platform answered but cannot scan; its reason is the
                // honest state word, not a transport failure.
                WireDiskState::Unavailable(reason) => DiskViewState::Failed(reason.clone()),
            }
        }
        Ok(_) => DiskViewState::Scanning,
        // An old connection's error says nothing about its replacement.
        Err(_) if !source.same_connection(&current) => DiskViewState::Scanning,
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => DiskViewState::Unsupported,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            missing_disk_source(host, cx)
        }
        Err(error) => DiskViewState::Failed(error.to_string()),
    }
}

/// The presentation-ready disk reading: the numbers, the directory line, the
/// state word and the hover detail, all resolved for one frame.
pub(crate) struct DiskReadout {
    size_bytes: Option<u64>,
    total_bytes: Option<u64>,
    partial: bool,
    directory: Option<String>,
    /// `Some(Pending)` is the "…" family, `Some(Down)` the "—" family.
    marker: Option<DiskMarker>,
    status: Option<String>,
    detail: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiskMarker {
    Pending,
    Down,
}

impl DiskReadout {
    fn pending(status: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            size_bytes: None,
            total_bytes: None,
            partial: false,
            directory: None,
            marker: Some(DiskMarker::Pending),
            status: Some(status.into()),
            detail: detail.into(),
        }
    }

    fn down(status: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            size_bytes: None,
            total_bytes: None,
            partial: false,
            directory: None,
            marker: Some(DiskMarker::Down),
            status: Some(status.into()),
            detail: detail.into(),
        }
    }

    fn ready(scan: &lyrel_core::host::disk::DirectoryScan, host: HostId, cx: &App) -> Self {
        let home = desktop_lookups::home_for_host(cx, host);
        let dir = std::path::PathBuf::from(scan.directory());
        let shown = display_path(&dir, home.as_deref());
        let size = gb(scan.size_bytes());
        let total = scan
            .volume_total_bytes()
            .map(|total| format!("{} GB", gb(total)))
            .unwrap_or_else(|| "—".into());
        let (detail, status) = if scan.is_partial() {
            let reason = scan
                .errors()
                .first()
                .cloned()
                .unwrap_or_else(|| t(L10nKey::MonitorDiskPartial).to_string());
            (
                t_fmt(
                    L10nKey::MonitorDiskPartialDetail,
                    &[
                        ("dir", &shown),
                        ("used", &format!("{size} GB")),
                        ("total", &total),
                        ("reason", &reason),
                    ],
                ),
                Some(t(L10nKey::MonitorDiskPartial).to_string()),
            )
        } else {
            (
                t_fmt(
                    L10nKey::MonitorDiskDetail,
                    &[
                        ("dir", &shown),
                        ("used", &format!("{size} GB")),
                        ("total", &total),
                    ],
                ),
                None,
            )
        };
        Self {
            size_bytes: Some(scan.size_bytes()),
            total_bytes: scan.volume_total_bytes(),
            partial: scan.is_partial(),
            directory: Some(shown),
            marker: None,
            status,
            detail,
        }
    }

    fn value_text(&self) -> String {
        match self.size_bytes {
            Some(size) => {
                // An unknown volume total shows "—" beside the size: the two
                // states mean different things and read differently.
                let total = self.total_bytes.map(gb).unwrap_or_else(|| "—".into());
                let text = format!("{} / {} GB", gb(size), total);
                if self.partial {
                    format!("≥ {text}")
                } else {
                    text
                }
            }
            None => match self.marker {
                Some(DiskMarker::Pending) => "…".into(),
                _ => "—".into(),
            },
        }
    }
}

fn gb(bytes: u64) -> String {
    let gib = bytes as f64 / 1_073_741_824.0;
    let text = format!("{gib:.1}");
    text.trim_end_matches(".0").to_owned()
}

impl LyrelApp {
    /// Keep exactly one scan view alive for the (host, directory) the
    /// monitor shows, and release it when the monitor hides or the pair
    /// changes. A pair change starts a fresh scan — never a re-poll on the
    /// metrics cadence. A route-missing state retries: the moment the link
    /// to the same pair exists again, the next sync replaces the view and
    /// its one scan runs. Failed and unsupported scans stay until the pair
    /// changes, so a dead directory is not rescanned every frame.
    pub(crate) fn sync_host_disk(&mut self, window: &Window, cx: &mut Context<Self>) {
        let monitor_shown =
            self.right_panel_visible && matches!(self.right_panel_tab, RightPanelTab::Monitor);
        let target = if monitor_shown {
            self.context_pane(window, cx).and_then(|leaf| {
                // `effective_host_cwd` is the directory the pane's work is
                // happening in, restricted to paths its host can act on —
                // exactly what a host-owned scan needs.
                let cwd = leaf.read(cx).effective_host_cwd()?;
                Some((leaf.read(cx).host_id(), cwd.to_string_lossy().into_owned()))
            })
        } else {
            None
        };
        let current = self.disk_view.take();
        self.disk_view = match (target, current) {
            (Some((host, dir)), Some(view)) => {
                let same = view.read(cx).host == host && view.read(cx).dir == dir;
                let retryable = matches!(
                    view.read(cx).state,
                    DiskViewState::Disconnected
                        | DiskViewState::Connecting
                        | DiskViewState::Reconnecting
                ) && MetricsSource::get(host, cx).is_some();
                if same && !retryable {
                    Some(view)
                } else {
                    Some(HostDiskView::acquire(host, dir, cx))
                }
            }
            (Some((host, dir)), None) => Some(HostDiskView::acquire(host, dir, cx)),
            (None, _) => None,
        };
    }

    /// The disk tile's reading for this frame: the view bound to the context
    /// pane's exact (host, directory), or the state word while there is
    /// nothing trustworthy to show.
    pub(crate) fn disk_readout(&self, window: &Window, cx: &mut Context<Self>) -> DiskReadout {
        let Some(pane) = self.context_pane(window, cx) else {
            return DiskReadout::pending(t(L10nKey::MonitorNoPane), t(L10nKey::MonitorNoPaneHint));
        };
        let Some(cwd) = pane.read(cx).effective_host_cwd() else {
            return DiskReadout::pending(
                t(L10nKey::PanelNoWorkingDirectory),
                t(L10nKey::PanelNoWorkingDirectoryHint),
            );
        };
        let host = pane.read(cx).host_id();
        let dir = cwd.to_string_lossy().into_owned();
        let Some(view) = self
            .disk_view
            .as_ref()
            .filter(|view| view.read(cx).host == host && view.read(cx).dir == dir)
        else {
            // The sync has not acquired this pair yet this frame: render the
            // in-flight state rather than a number that belongs to nobody.
            return DiskReadout::pending(t(L10nKey::MonitorDiskScanning), dir);
        };
        let state = view.update(cx, |view, cx| view.effective_state(cx));
        match state {
            DiskViewState::Ready(usage) => match usage.state() {
                WireDiskState::Scanned(scan) => DiskReadout::ready(scan, host, cx),
                WireDiskState::Unavailable(reason) => {
                    DiskReadout::down(reason.clone(), reason.clone())
                }
            },
            DiskViewState::Scanning => DiskReadout::pending(t(L10nKey::MonitorDiskScanning), dir),
            DiskViewState::Connecting => DiskReadout::pending(
                t(L10nKey::HostMetricsConnecting),
                t(L10nKey::HostMetricsConnecting),
            ),
            DiskViewState::Reconnecting => DiskReadout::pending(
                t(L10nKey::HostMetricsReconnecting),
                t(L10nKey::HostMetricsReconnecting),
            ),
            DiskViewState::Disconnected => DiskReadout::down(
                t(L10nKey::HostMetricsDisconnected),
                t(L10nKey::HostMetricsDisconnected),
            ),
            DiskViewState::Unsupported => DiskReadout::down(
                t(L10nKey::MonitorDiskUnavailable),
                t(L10nKey::MonitorDiskUnavailable),
            ),
            DiskViewState::Failed(reason) => {
                let text = t_fmt(L10nKey::MonitorDiskFailed, &[("reason", &reason)]);
                DiskReadout::down(text.clone(), text)
            }
        }
    }

    /// The Activity Monitor's disk tile, in the approved layout: label and
    /// format hint, the latest completed scan's "size / total", and the
    /// scanned directory underneath in mono. No chart — a scan is a snapshot,
    /// not a series.
    pub(crate) fn render_disk_tile(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let readout = self.disk_readout(window, cx);
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let value = readout.value_text();
        let detail = readout.detail.clone();
        v_flex()
            .id("monitor-tile-disk")
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
                            .text_color(theme.info)
                            .child(t(L10nKey::MonitorColDisk)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .text_size(rems(11. / 16.))
                            .text_color(theme.muted_foreground)
                            .child(t(L10nKey::MonitorDiskHint)),
                    )
                    .when_some(readout.status, |row, status| {
                        row.child(
                            div()
                                .min_w_0()
                                .text_ellipsis()
                                .text_size(rems(11. / 16.))
                                .text_color(theme.muted_foreground)
                                .child(status),
                        )
                    }),
            )
            .child(
                div()
                    .id("monitor-tile-disk-value")
                    .aria_label(gpui_kit::SharedString::from(value.clone()))
                    .test_support()
                    .text_size(rems(17. / 16.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(gpui_kit::SharedString::from(value)),
            )
            .when_some(readout.directory, |tile, directory| {
                tile.child(
                    div()
                        .id("monitor-tile-disk-dir")
                        .aria_label(gpui_kit::SharedString::from(directory.clone()))
                        .test_support()
                        .min_w_0()
                        .text_ellipsis()
                        .font_family(mono)
                        .text_size(rems(11. / 16.))
                        .text_color(theme.muted_foreground)
                        .child(gpui_kit::SharedString::from(directory)),
                )
            })
            .tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "host_disk_tests.rs"]
pub(crate) mod tests;
