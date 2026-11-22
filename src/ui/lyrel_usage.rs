//! The Lyrel component-usage row below the Activity Monitor's resource
//! tiles (#377).
//!
//! Collapsed, it is one summary line: the local desktop application plus the
//! local session daemon's CPU and RAM, with an "includes remote" badge when
//! the context pane lives on a managed remote. Expanded, it lists the
//! components grouped by host — the desktop application and the session
//! daemon under the local host, the remote Lyrel server under its own.
//!
//! Every number is bound to the process and daemon instance that produced
//! it: the daemon side stamps its answer with the control instance and the
//! process birth time, and this module revalidates the stamp against the
//! live connection before painting, so a replaced daemon or a dropped link
//! downgrades the row to its pending or disconnected marker instead of
//! replaying stale readings. The desktop application samples its own process
//! in place — it cannot answer for anyone else, and no wire crosses for it.
//!
//! Shells and agents stay out of these totals by construction: the daemon
//! sampler refreshes exactly one process (itself), never the machine's
//! table, so a workload spawned under a pane can never leak in. The title
//! bar never mounts this view; it keeps reading whole-host samples only.
use std::sync::{Arc, Mutex};

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{Icon, IconName, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Render, Styled as _, div,
    prelude::*, px, rems,
};
use lyrel_core::host::{
    Host as _, HostId,
    lyrel_usage::{ComponentUsage, LyrelUsage, LyrelUsageState, ProcessMetricUnavailable, Sampler},
    metrics::HostMetrics,
    remote::RemoteHost,
};

use crate::ui::app::LyrelApp;
use crate::ui::desktop_lookups::HostLinks;
use crate::ui::local_link::LocalLink;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

/// The row's own poll cadence, matching the whole-host sampler: one second
/// between asks while the monitor shows, nothing at all when it does not.
const SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Cell widths, aligned with the process table's columns below so the
/// expanded rows read on one grid.
const CPU_W: f32 = 40.;
const MEM_W: f32 = 52.;
const GPU_W: f32 = 36.;
const NET_W: f32 = 84.;

/// The answer from one daemon-shaped source: the local daemon or the remote
/// Lyrel server. Both speak through the same `HostMetrics` response, so one
/// slot shape covers them.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    /// No trustworthy answer yet — priming, or the source was replaced and
    /// the next poll has not landed.
    Priming,
    /// A sample whose instance still matches its source at paint time.
    Ready(LyrelUsage),
    /// The source cannot attribute Lyrel usage (a peer from before the
    /// feature), or says why itself.
    Unsupported,
    /// The source's link is down; readings stop rather than age on screen.
    Gone,
    Failed(String),
}

impl Slot {
    fn component(&self) -> Option<&ComponentUsage> {
        let Self::Ready(usage) = self else {
            return None;
        };
        match usage.state() {
            LyrelUsageState::Sample(sample) => Some(sample.server()),
            _ => None,
        }
    }
}

/// The desktop application's own reading. `Priming` is the CPU baseline
/// pass; the process cannot disconnect from itself.
#[derive(Clone, Debug, PartialEq)]
enum Desktop {
    Priming,
    Ready(ComponentUsage),
    Failed(String),
}

/// One presentation cell: the value pair when both halves exist, otherwise
/// the marker the state word justifies.
#[derive(Clone, Copy, PartialEq)]
struct Cell {
    cpu: Option<f32>,
    memory: Option<u64>,
    pending: bool,
}

impl Cell {
    fn of_slot(slot: &Slot) -> Self {
        match slot {
            Slot::Ready(usage) => match usage.state() {
                LyrelUsageState::Sample(sample) => Self {
                    cpu: Some(sample.server().cpu_percent()),
                    memory: Some(sample.server().memory_used_bytes()),
                    pending: false,
                },
                LyrelUsageState::Waiting => Self::pending(),
                LyrelUsageState::Unavailable(_) => Self::down(),
            },
            Slot::Priming => Self::pending(),
            Slot::Unsupported | Slot::Gone | Slot::Failed(_) => Self::down(),
        }
    }

    fn of_desktop(desktop: &Desktop) -> Self {
        match desktop {
            Desktop::Ready(component) => Self {
                cpu: Some(component.cpu_percent()),
                memory: Some(component.memory_used_bytes()),
                pending: false,
            },
            Desktop::Priming => Self::pending(),
            Desktop::Failed(_) => Self::down(),
        }
    }

    fn pending() -> Self {
        Self {
            cpu: None,
            memory: None,
            pending: true,
        }
    }

    fn down() -> Self {
        Self {
            cpu: None,
            memory: None,
            pending: false,
        }
    }

    /// Sum of two component cells: the collapsed local aggregate. It exists
    /// only while both halves are real readings; one pending half makes the
    /// sum pending rather than half a number, and one down half makes it
    /// down rather than a lie about the total.
    fn add(self, other: Self) -> Self {
        match (self.cpu, other.cpu) {
            (Some(a), Some(b)) => Self {
                cpu: Some(a + b),
                memory: match (self.memory, other.memory) {
                    (Some(x), Some(y)) => Some(x + y),
                    _ => None,
                },
                pending: false,
            },
            _ if self.pending || other.pending => Self::pending(),
            _ => Self::down(),
        }
    }
}

/// The demand-driven sampler view for the Lyrel row. Exactly one consumer —
/// the monitor's resource section — holds it, so its loop polls only while
/// that section is on screen.
pub(crate) struct LyrelUsageView {
    metric_host: HostId,
    desktop: Desktop,
    local: Slot,
    remote: Option<(HostId, Slot)>,
    expanded: bool,
}

impl LyrelUsageView {
    fn new(metric_host: HostId, cx: &mut Context<Self>) -> Self {
        let desktop_sampler: Arc<Mutex<Option<Sampler>>> = Arc::new(Mutex::new(None));
        cx.spawn(async move |this, cx| {
            loop {
                if this.upgrade().is_none() {
                    break;
                }
                enum Wire {
                    Local(Arc<lyrel_core::daemon::control::ControlClient>),
                    Remote(Arc<RemoteHost>),
                }
                impl Wire {
                    fn ask(&self) -> std::io::Result<HostMetrics> {
                        match self {
                            Self::Local(client) => client.host_metrics(),
                            Self::Remote(host) => host.host_metrics(),
                        }
                    }
                    fn instance(&self) -> &str {
                        match self {
                            Self::Local(client) => &client.hello().instance,
                            Self::Remote(host) => &host.peer().instance,
                        }
                    }
                }

                let (local, remote) = cx.update(|cx| {
                    (
                        LocalLink::client(cx).map(Wire::Local),
                        (!metric_host.is_local())
                            .then(|| HostLinks::get(cx, metric_host))
                            .flatten()
                            .map(Wire::Remote),
                    )
                });
                let desktop_sampler = desktop_sampler.clone();
                let Some((desktop, local, remote)) =
                    lyrel_ui_base::host_ops::off_thread(move || {
                        let desktop = {
                            let mut guard = desktop_sampler
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            match guard.as_mut() {
                                None => match Sampler::for_pid(std::process::id()) {
                                    Ok(sampler) => {
                                        *guard = Some(sampler);
                                        Desktop::Priming
                                    }
                                    Err(error) => Desktop::Failed(error.to_string()),
                                },
                                Some(sampler) => match sampler.sample() {
                                    Ok(Some(component)) => Desktop::Ready(component),
                                    Ok(None) => Desktop::Priming,
                                    Err(error) => Desktop::Failed(error.to_string()),
                                },
                            }
                        };
                        (
                            desktop,
                            local.map(|wire| {
                                let instance = wire.instance().to_owned();
                                (wire.ask(), instance)
                            }),
                            remote.map(|wire| {
                                let instance = wire.instance().to_owned();
                                (wire.ask(), instance)
                            }),
                        )
                    })
                    .await
                else {
                    break;
                };
                if this
                    .update(cx, |view, cx| {
                        view.desktop = desktop;
                        if let Some((result, instance)) = local {
                            view.local = slot_of(result, &instance);
                        } else {
                            view.local = Slot::Gone;
                        }
                        if let Some((host, slot)) = view
                            .remote
                            .as_ref()
                            .map(|(host, _)| *host)
                            .zip(remote.map(|(result, instance)| slot_of(result, &instance)))
                        {
                            view.remote = Some((host, slot));
                        }
                        cx.notify();
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
            metric_host,
            desktop: Desktop::Priming,
            local: Slot::Priming,
            remote: (!metric_host.is_local()).then(|| (metric_host, Slot::Priming)),
            expanded: false,
        }
    }

    /// The render-time guard a `Ready` sample carries: the source must still
    /// be connected for the same daemon instance, otherwise the reading
    /// downgrades before the next poll could — a disconnect never displays
    /// stale numbers, not even for a frame.
    fn effective(&self, cx: &mut App) -> (Desktop, Slot, Option<(HostId, Slot)>) {
        let local = match &self.local {
            Slot::Ready(usage) => match LocalLink::client(cx) {
                Some(client) if client.hello().instance == usage.instance() => self.local.clone(),
                // A replacement client speaking for a new daemon instance
                // must present its own sample; until its poll lands, the old
                // reading stays off screen.
                Some(_) => Slot::Priming,
                None => Slot::Gone,
            },
            other => other.clone(),
        };
        let remote = self.remote.as_ref().map(|(host, slot)| {
            let slot = match slot {
                Slot::Ready(usage) => match HostLinks::get(cx, *host) {
                    Some(link) if link.peer().instance == usage.instance() => slot.clone(),
                    Some(_) => Slot::Priming,
                    None => Slot::Gone,
                },
                other => other.clone(),
            };
            (*host, slot)
        });
        (self.desktop.clone(), local, remote)
    }
}

/// Map one daemon answer onto a slot, binding it to the instance the source
/// hello advertised. An answer stamped for another daemon is dropped to
/// priming: the source will present its own reading on the next poll.
fn slot_of(result: std::io::Result<HostMetrics>, instance: &str) -> Slot {
    match result {
        Ok(metrics) => match metrics.lyrel_usage() {
            Some(usage) if usage.instance() == instance => Slot::Ready(usage.clone()),
            Some(_) => Slot::Priming,
            None => Slot::Unsupported,
        },
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => Slot::Unsupported,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            Slot::Gone
        }
        Err(error) => Slot::Failed(error.to_string()),
    }
}

impl Render for LyrelUsageView {
    // The view owns the poll; the row paints from `LyrelApp::render_lyrel_usage`.
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn format_cpu(cpu: f32) -> String {
    match cpu < 10.0 {
        true => format!("{cpu:.1}%"),
        false => format!("{cpu:.0}%"),
    }
}

fn network_pair_text(rx: f64, tx: f64) -> String {
    let rx = super::host_metrics::rate_text(rx);
    let tx = super::host_metrics::rate_text(tx);
    if let (Some((received, rx_unit)), Some((_, tx_unit))) =
        (rx.rsplit_once(' '), tx.rsplit_once(' '))
        && rx_unit == tx_unit
    {
        return format!("{received} / {tx}");
    }
    format!("{rx} / {tx}")
}

/// One right-aligned value cell, the width the process-table columns use.
fn usage_cell(text: String, width: f32, mono: gpui_kit::SharedString) -> gpui_kit::Div {
    div()
        .w(px(width))
        .flex_none()
        .text_right()
        .text_size(rems(11. / 16.))
        .font_family(mono)
        .child(gpui_kit::SharedString::from(text))
}

fn cell_text(cell: &Cell) -> (String, String, &'static str) {
    let marker = match cell.pending {
        true => "…",
        false => "—",
    };
    (
        cell.cpu.map(format_cpu).unwrap_or_else(|| marker.into()),
        cell.memory
            .map(crate::core::proc::compact_bytes)
            .unwrap_or_else(|| marker.into()),
        marker,
    )
}

fn busiest_gpu(desktop: &[(String, f32)], daemon: &[(String, f32)]) -> f32 {
    let mut engines = std::collections::HashMap::<&str, f32>::new();
    for (engine, percent) in desktop.iter().chain(daemon) {
        *engines.entry(engine).or_default() += percent;
    }
    engines.into_values().fold(0., f32::max).min(100.)
}

fn unavailable_detail(metric: &'static str, reason: Option<ProcessMetricUnavailable>) -> String {
    let detail = t(match reason {
        Some(ProcessMetricUnavailable::Unsupported) => L10nKey::LyrelProcessMetricUnsupported,
        Some(ProcessMetricUnavailable::CounterReadFailed) => L10nKey::LyrelProcessMetricFailed,
        None => L10nKey::LyrelUsageUnavailable,
    });
    format!("{metric} · {detail}")
}

impl LyrelApp {
    /// Keep exactly one Lyrel usage sampler alive for the host the monitor
    /// shows, and release it when the monitor leaves the screen. Called from
    /// the same per-frame composition pass as the whole-host sync; the two
    /// samplers stay independent so the title bar never pays for this poll.
    pub(crate) fn sync_lyrel_usage(&mut self, window: &gpui_kit::Window, cx: &mut Context<Self>) {
        let host = self.metric_host(window, cx);
        let monitor_shown = self.right_panel_visible
            && matches!(
                self.right_panel_tab,
                lyrel_core::core::config::RightPanelTab::Monitor
            )
            && self.right_panel.procs_pane.is_some();
        let current = self.lyrel_usage_view.take();
        self.lyrel_usage_view = match (monitor_shown, current) {
            (true, Some(view)) if view.read(cx).metric_host == host => Some(view),
            (true, _) => Some(cx.new(|cx| LyrelUsageView::new(host, cx))),
            (false, _) => None,
        };
    }

    /// The expandable Lyrel summary row under the monitor's resource tiles.
    pub(crate) fn render_lyrel_usage(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.lyrel_usage_view.clone() else {
            return div().id("lyrel-usage").test_support().into_any_element();
        };
        let (desktop, local, remote) = view.update(cx, |view, cx| view.effective(cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let expanded = view.read(cx).expanded;
        let mono = theme.mono_font_family.clone();

        let desktop_cell = Cell::of_desktop(&desktop);
        let local_cell = Cell::of_slot(&local);
        let local_total = desktop_cell.add(local_cell);
        let remote_cell = remote
            .as_ref()
            .map(|(_, slot)| Cell::of_slot(slot))
            .unwrap_or_else(Cell::down);
        let desktop_component = match &desktop {
            Desktop::Ready(component) => Some(component),
            _ => None,
        };
        let summary_gpu = match (
            desktop_component.and_then(ComponentUsage::gpu),
            local.component().and_then(ComponentUsage::gpu),
        ) {
            (Some(Ok(ui)), Some(Ok(daemon))) => format_cpu(busiest_gpu(ui, daemon)),
            _ if local_total.pending => "…".into(),
            _ => "—".into(),
        };

        let (cpu_text, mem_text, _) = cell_text(&local_total);
        let local_name = match &local {
            Slot::Ready(usage) => usage.hostname(),
            _ => t(L10nKey::LyrelUsageLocal),
        };
        let remote_host = remote
            .as_ref()
            .and_then(|(_, slot)| match slot {
                Slot::Ready(usage) => Some(usage.hostname().to_owned()),
                _ => None,
            })
            .unwrap_or_default();
        let has_remote = remote.is_some();
        let includes_remote = has_remote && (remote_cell.cpu.is_some() || remote_cell.pending);

        let summary_detail = t_fmt(
            L10nKey::LyrelUsageSample,
            &[
                ("host", local_name),
                ("cpu", &cpu_text),
                ("mem", &mem_text),
                ("gpu", &summary_gpu),
            ],
        );

        let summary = h_flex()
            .id("lyrel-usage-summary")
            .test_support()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .aria_label(gpui_kit::SharedString::from(summary_detail))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(view) = this.lyrel_usage_view.as_ref() {
                    view.update(cx, |view, cx| {
                        view.expanded = !view.expanded;
                        cx.notify();
                    });
                }
            }))
            .child(
                div()
                    .id("lyrel-usage-expand")
                    .test_support()
                    .flex_none()
                    .w(px(10.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .role(gpui_kit::Role::Button)
                    .aria_label(t(if expanded {
                        L10nKey::LyrelUsageCollapse
                    } else {
                        L10nKey::LyrelUsageExpand
                    }))
                    .child(
                        Icon::new(match expanded {
                            true => IconName::ChevronDown,
                            false => IconName::ChevronRight,
                        })
                        .size(px(10.))
                        .text_color(muted),
                    ),
            )
            .child(
                div()
                    .text_size(rems(11. / 16.))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child("Lyrel"),
            )
            .child(
                div()
                    .id("lyrel-usage-host")
                    .test_support()
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme.muted.opacity(0.35))
                    .text_size(rems(10. / 16.))
                    .text_color(muted)
                    .child(t(L10nKey::LyrelUsageLocal)),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .id("lyrel-usage-cpu")
                            .test_support()
                            .text_size(rems(11. / 16.))
                            .font_family(mono.clone())
                            .aria_label(gpui_kit::SharedString::from(format!("CPU {cpu_text}")))
                            .child(gpui_kit::SharedString::from(format!("CPU {cpu_text}"))),
                    )
                    .child(
                        div()
                            .id("lyrel-usage-ram")
                            .test_support()
                            .text_size(rems(11. / 16.))
                            .font_family(mono.clone())
                            .aria_label(gpui_kit::SharedString::from(format!("RAM {mem_text}")))
                            .child(gpui_kit::SharedString::from(format!("RAM {mem_text}"))),
                    )
                    .child(
                        div()
                            .id("lyrel-usage-gpu")
                            .test_support()
                            .text_size(rems(11. / 16.))
                            .font_family(mono.clone())
                            .aria_label(gpui_kit::SharedString::from(format!("GPU {summary_gpu}")))
                            .child(format!("GPU {summary_gpu}")),
                    ),
            )
            .when(includes_remote, |row| {
                row.child(
                    div()
                        .id("lyrel-usage-includes-remote")
                        .test_support()
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(theme.accent.opacity(0.14))
                        .text_size(rems(10. / 16.))
                        .text_color(theme.accent)
                        .child(t(L10nKey::LyrelUsageIncludesRemote)),
                )
            })
            .into_any_element();

        let mut body = v_flex()
            .id("lyrel-usage")
            .test_support()
            .px(px(crate::ui::window_chrome::CONTENT_INSET))
            .pt(px(6.))
            .child(summary);

        if !expanded {
            return body.into_any_element();
        }

        // The expanded grid: one column pair per metric, the same right-aligned
        // widths the process table uses, so the whole monitor reads on one grid.
        let title = |label: &'static str, width: f32| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .text_size(rems(10. / 16.))
                .text_color(muted)
                .child(label)
        };
        let header = h_flex()
            .pb(px(2.))
            .gap(px(4.))
            .child(div().flex_1().min_w_0())
            .child(title("CPU", CPU_W))
            .child(title("RAM", MEM_W))
            .child(title("GPU", GPU_W))
            .child(title(t(L10nKey::LyrelProcessNetwork), NET_W));
        body = body.child(header);

        let group = |title: String| {
            div()
                .pt(px(4.))
                .pb(px(1.))
                .pl(px(16.))
                .text_size(rems(10. / 16.))
                .text_color(muted)
                .child(title)
        };
        let local_group_title = t(L10nKey::LyrelUsageLocal).to_string();
        let remote_group_title = t_fmt(L10nKey::LyrelUsageRemote, &[("host", &remote_host)]);
        let row = |id: &'static str,
                   label: String,
                   cell: Cell,
                   component: Option<&ComponentUsage>| {
            let (cpu, mem, marker) = cell_text(&cell);
            let gpu = component.and_then(ComponentUsage::gpu);
            let net = component.and_then(ComponentUsage::network);
            let gpu_text = match gpu {
                Some(Ok(engines)) => format_cpu(busiest_gpu(engines, &[])),
                _ => marker.into(),
            };
            let network_text = match net {
                Some(Ok(rate)) => {
                    network_pair_text(rate.rx_bytes_per_sec(), rate.tx_bytes_per_sec())
                }
                _ => marker.into(),
            };
            let gpu_detail = match gpu {
                Some(Ok(_)) => format!("GPU {gpu_text}"),
                _ => unavailable_detail("GPU", gpu.and_then(Result::err)),
            };
            let network_detail = match net {
                Some(Ok(_)) => format!("{} {network_text}", t(L10nKey::LyrelProcessNetwork)),
                _ => unavailable_detail(t(L10nKey::MonitorColNetwork), net.and_then(Result::err)),
            };
            let detail = match (&cell.cpu, &cell.memory) {
                (Some(_), Some(_)) => t_fmt(
                    L10nKey::LyrelUsageComponent,
                    &[("name", &label), ("cpu", &cpu), ("mem", &mem)],
                ),
                _ if cell.pending => t(L10nKey::LyrelUsageWaiting).to_string(),
                _ => t(L10nKey::LyrelUsageUnavailable).to_string(),
            };
            h_flex()
                .id(id)
                .test_support()
                .items_center()
                .gap(px(4.))
                .cursor_default()
                .aria_label(gpui_kit::SharedString::from(detail))
                .child(
                    div()
                        .id(format!("{id}-name"))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(rems(11. / 16.))
                        .child(label),
                )
                .child(
                    usage_cell(cpu.clone(), CPU_W, mono.clone())
                        .id(format!("{id}-cpu"))
                        .aria_label(gpui_kit::SharedString::from(cpu))
                        .test_support(),
                )
                .child(
                    usage_cell(mem.clone(), MEM_W, mono.clone())
                        .id(format!("{id}-mem"))
                        .aria_label(gpui_kit::SharedString::from(mem))
                        .test_support(),
                )
                .child(
                    usage_cell(gpu_text, GPU_W, mono.clone())
                        .id(format!("{id}-gpu"))
                        .aria_label(gpui_kit::SharedString::from(gpu_detail.clone()))
                        .test_support()
                        .tooltip(move |window, cx| {
                            Tooltip::new(gpu_detail.clone()).build(window, cx)
                        }),
                )
                .child(
                    usage_cell(network_text, NET_W, mono.clone())
                        .id(format!("{id}-network"))
                        .aria_label(gpui_kit::SharedString::from(network_detail.clone()))
                        .test_support()
                        .tooltip(move |window, cx| {
                            Tooltip::new(network_detail.clone()).build(window, cx)
                        }),
                )
                .into_any_element()
        };

        body = body
            .child(
                v_flex()
                    .id("lyrel-usage-group-local")
                    .test_support()
                    .aria_label(gpui_kit::SharedString::from(local_group_title.clone()))
                    .child(group(local_group_title))
                    .child(row(
                        "lyrel-usage-desktop",
                        t(L10nKey::LyrelUsageDesktop).to_string(),
                        desktop_cell,
                        desktop_component,
                    ))
                    .child(row(
                        "lyrel-usage-daemon",
                        t(L10nKey::LyrelUsageDaemon).to_string(),
                        local_cell,
                        local.component(),
                    )),
            )
            .when(has_remote, |body| {
                body.child(
                    v_flex()
                        .id("lyrel-usage-group-remote")
                        .test_support()
                        .aria_label(gpui_kit::SharedString::from(remote_group_title.clone()))
                        .child(group(remote_group_title))
                        .child(row(
                            "lyrel-usage-server",
                            t(L10nKey::LyrelUsageServer).to_string(),
                            remote_cell,
                            remote.as_ref().and_then(|(_, slot)| slot.component()),
                        )),
                )
            });

        body.into_any_element()
    }
}

#[cfg(test)]
#[path = "lyrel_usage_tests.rs"]
pub(crate) mod tests;
