//! 所选机器的原生历史总览；所有文件与聚合离开 GPUI 线程。
mod analysis;
mod cache;
mod dashboard;
mod management;
#[cfg(test)]
mod management_tests;
mod storage;
#[cfg(test)]
mod workflow_tests;
use super::kit::Tk;
use super::shell::MenuEntry;
use super::*;
use crate::ui::host_ops::HostOps;
use crate::ui::host_registry::HostRegistry;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{Disableable, button::ButtonVariants, spinner::Spinner};
use lyrel_core::statistics::{self, DateWindow, Period, Snapshot, StatisticsCache, Totals};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static INTERACTIVE_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatisticsRequest {
    Load,
    Refresh,
    Recompute,
}

pub(crate) fn schedule_local_warmup(cx: &mut App) {
    static SCHEDULED: AtomicBool = AtomicBool::new(false);
    if SCHEDULED.swap(true, Ordering::Relaxed) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(5))
            .await;
        let request = cx.update(|cx| {
            if INTERACTIVE_REQUESTED.load(Ordering::Relaxed)
                || crate::ui::windows::WindowRegistry::count(cx) == 0
            {
                return None;
            }
            Some(cache::Request {
                host: HostRegistry::local(cx),
                agent: None,
                launch: cx.global::<Config>().agent_launch.clone(),
                stored: Arc::default(),
                path: Some(crate::core::config::config_path(
                    "statistics-settings.json",
                )?),
                opencode_sources: Vec::new(),
                reuse_snapshot: false,
            })
        });
        if let Some(request) = request {
            cx.background_spawn(async move {
                // Dropping this subscriber does not cancel the shared publication.
                // No catalog HTTP, account request, or agent mutation occurs here.
                let _ = cache::shared().subscribe(request, false);
            })
            .await;
        }
    })
    .detach();
}

pub(crate) struct StatisticsState {
    pub(crate) host: HostId,
    content_scroll: gpui_kit::ScrollHandle,
    agent: Option<&'static str>,
    period: Period,
    requested_window: Option<DateWindow>,
    start: Entity<InputState>,
    end: Entity<InputState>,
    generation: Uuid,
    scan_generation: Option<Uuid>,
    shared_revision: Option<Uuid>,
    aggregate_version: Arc<AtomicU64>,
    loading: bool,
    error: Option<String>,
    pending_refresh: bool,
    results: HashMap<(HostId, Option<&'static str>), Presentation>,
    analysis: analysis::AnalysisState,
    management: management::Management,
}
struct Presentation {
    snapshot: Arc<Snapshot>,
    window: DateWindow,
    totals: Totals,
    previous: Option<Totals>,
    analysis: Option<statistics::analysis::Analysis>,
    analysis_error: Option<String>,
    bindings: Arc<[statistics::sources::SourceBinding]>,
}
impl Presentation {
    fn from_snapshot(
        snapshot: Arc<Snapshot>,
        bindings: impl Into<Arc<[statistics::sources::SourceBinding]>>,
        window: DateWindow,
        analysis_agent: Option<&str>,
        year: i32,
    ) -> Self {
        let totals = snapshot.aggregate(&window);
        let previous = window.previous().map(|window| snapshot.aggregate(window));
        let (analysis, analysis_error) =
            match snapshot.analyze(&window, chrono::Local::now(), analysis_agent, year) {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(error)),
            };
        Self {
            snapshot,
            bindings: bindings.into(),
            window,
            totals,
            previous,
            analysis,
            analysis_error,
        }
    }
}
impl StatisticsState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        Self {
            host: HostId::LOCAL,
            content_scroll: gpui_kit::ScrollHandle::new(),
            agent: None,
            period: Period::Month,
            requested_window: None,
            start: cx.new(|cx| InputState::new(window, cx).default_value(date.clone())),
            end: cx.new(|cx| InputState::new(window, cx).default_value(date)),
            generation: Uuid::new_v4(),
            scan_generation: None,
            shared_revision: None,
            aggregate_version: Arc::new(AtomicU64::new(0)),
            loading: false,
            error: None,
            pending_refresh: false,
            results: HashMap::new(),
            analysis: analysis::AnalysisState::new(cx),
            management: management::Management::new(window, cx),
        }
    }
    fn invalidate_view(&mut self) {
        self.generation = Uuid::new_v4();
        self.aggregate_version.fetch_add(1, Ordering::Relaxed);
        self.loading = self.scan_generation.is_some();
        self.requested_window = None;
    }
    pub(crate) fn invalidate(&mut self) {
        self.generation = Uuid::new_v4();
        self.aggregate_version.fetch_add(1, Ordering::Relaxed);
        self.scan_generation = None;
        self.shared_revision = None;
        self.loading = false;
        self.requested_window = None;
    }
}
fn period_key(period: Period) -> L10nKey {
    match period {
        Period::Today => L10nKey::StatisticsToday,
        Period::LastSevenDays => L10nKey::StatisticsLastSevenDays,
        Period::Week => L10nKey::StatisticsWeek,
        Period::Month => L10nKey::StatisticsMonth,
        Period::Year => L10nKey::StatisticsYear,
        Period::All => L10nKey::StatisticsAll,
        Period::Custom => L10nKey::StatisticsCustom,
    }
}
fn number(value: Option<u64>) -> String {
    value
        .map(|v| group_thousands(v as usize))
        .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into())
}
fn session_count(totals: &Totals) -> String {
    number(
        (totals.main_sessions() > 0 || totals.calls() > 0 || totals.is_complete())
            .then_some(totals.main_sessions()),
    )
}
fn money(value: Option<f64>) -> String {
    value
        .map(|v| format!("USD {v:.4}"))
        .unwrap_or_else(|| t(L10nKey::StatisticsUnpriced).into())
}
fn issue_text(reason: &str) -> String {
    let key = if reason.ends_with(" source is still scanning") {
        Some(L10nKey::StatisticsLoading)
    } else if reason.starts_with("Hermes public projects.tree excludes") {
        Some(L10nKey::StatisticsHermesCoverage)
    } else if reason.starts_with("Hermes empty response") {
        Some(L10nKey::StatisticsHermesEmpty)
    } else if reason.starts_with("Hermes global 501-session")
        || reason.starts_with("Hermes project preview window")
    {
        Some(L10nKey::StatisticsHermesWindow)
    } else if reason.starts_with("Hermes response profile differs") {
        Some(L10nKey::StatisticsHermesProfile)
    } else if reason.starts_with("Hermes overlapping native lineage") {
        Some(L10nKey::StatisticsHermesLineage)
    } else if reason.starts_with("Hermes stored usage has no reliable") {
        Some(L10nKey::StatisticsHermesTime)
    } else {
        None
    };
    key.map(|key| t(key).to_string())
        .unwrap_or_else(|| reason.to_owned())
}
impl SettingsWindow {
    pub(crate) fn load_statistics(&mut self, cx: &mut Context<Self>) {
        self.request_statistics(StatisticsRequest::Load, cx);
    }
    fn refresh_statistics(&mut self, cx: &mut Context<Self>) {
        self.request_statistics(StatisticsRequest::Refresh, cx);
    }
    fn recompute_statistics(&mut self, cx: &mut Context<Self>) {
        self.request_statistics(StatisticsRequest::Recompute, cx);
    }
    fn request_statistics(&mut self, kind: StatisticsRequest, cx: &mut Context<Self>) {
        let refresh = kind == StatisticsRequest::Refresh;
        if !self
            .active_settings()
            .is_some_and(|settings| settings.navigation.section() == SettingsSection::Statistics)
        {
            return;
        }
        INTERACTIVE_REQUESTED.store(true, Ordering::Relaxed);
        if refresh {
            if let Some(settings) = self.active_settings_mut() {
                settings.statistics.pending_refresh = true;
            }
        }
        if !self.ensure_statistics_settings(cx) {
            return;
        }
        let Some(settings) = self.active_settings() else {
            return;
        };
        if settings.navigation.section() != SettingsSection::Statistics {
            return;
        }
        let state = &settings.statistics;
        if state.management.busy {
            return;
        }
        let custom = if state.period == Period::Custom {
            let start = chrono::NaiveDate::parse_from_str(
                state.start.read(cx).value().as_str(),
                "%Y-%m-%d",
            );
            let end =
                chrono::NaiveDate::parse_from_str(state.end.read(cx).value().as_str(), "%Y-%m-%d");
            match (start, end) {
                (Ok(start), Ok(end)) => Some((start, end)),
                _ => {
                    if let Some(s) = self.active_settings_mut() {
                        s.statistics.invalidate_view();
                        s.statistics.error = Some(t(L10nKey::StatisticsInvalidDate).into());
                    }
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let window = match DateWindow::calendar(state.period, chrono::Local::now(), custom) {
            Ok(window) => window,
            Err(_) => {
                if let Some(s) = self.active_settings_mut() {
                    s.statistics.invalidate_view();
                    s.statistics.error = Some(t(L10nKey::StatisticsInvalidDate).into());
                }
                cx.notify();
                return;
            }
        };
        let host_id = state.host;
        let agent = state.agent;
        let refresh = refresh || state.pending_refresh;
        let launch = cx.global::<Config>().agent_launch.clone();
        let stored = state.management.stored.clone();
        let opencode_sources = if agent.is_none_or(|agent| agent == "opencode") {
            (self.workspace.session_sources)(host_id, cx)
        } else {
            Vec::new()
        };
        let path = state.management.path.clone();
        let Some(host) = HostRegistry::get(cx, host_id) else {
            if let Some(s) = self.active_settings_mut() {
                s.statistics.invalidate();
                s.statistics.error = Some(t(L10nKey::StatisticsOffline).into());
            }
            cx.notify();
            return;
        };
        let generation = Uuid::new_v4();
        if let Some(s) = self.active_settings_mut() {
            s.statistics.generation = generation;
            s.statistics
                .aggregate_version
                .fetch_add(1, Ordering::Relaxed);
            s.statistics.scan_generation = Some(generation);
            s.statistics.pending_refresh = false;
            s.statistics.loading = true;
            s.statistics.requested_window = Some(window.clone());
            s.statistics.error = None;
            if s.statistics.analysis.anchor.is_none() {
                s.statistics.analysis.anchor = Some(gpui_kit::ScrollAnchor::for_handle(
                    s.statistics.content_scroll.clone(),
                ));
            }
        }
        let expected_host = host.clone();
        let expected_launch = launch.clone();
        let request = cache::Request {
            host,
            agent,
            launch,
            stored,
            path,
            opencode_sources,
            reuse_snapshot: kind == StatisticsRequest::Recompute,
        };
        cx.spawn(async move |this, cx| {
            let subscription = cx
                .background_spawn(async move { cache::shared().subscribe(request, refresh) })
                .await;
            let receiver = match subscription {
                Ok(receiver) => receiver,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.publish_statistics_error(host_id, agent, generation, error, cx);
                    });
                    return;
                }
            };
            while let Ok(update) = receiver.recv().await {
                let finished = update.finished;
                let active = this
                    .update(cx, |this, cx| {
                        if !this.active_settings().is_some_and(|settings| {
                            settings.navigation.section() == SettingsSection::Statistics
                                && settings.statistics.scan_generation == Some(generation)
                        }) {
                            return false;
                        }
                        let current_host = HostRegistry::lookup(cx, host_id);
                        let valid = current_host
                            .as_ref()
                            .is_some_and(|host| Arc::ptr_eq(host, &expected_host))
                            && cx.global::<Config>().agent_launch == expected_launch
                            && cache::shared().accepts(host_id, agent, update.revision);
                        if !valid {
                            if let Some(settings) = this.active_settings_mut() {
                                if settings.statistics.scan_generation == Some(generation) {
                                    settings.statistics.invalidate();
                                }
                            }
                            this.load_statistics(cx);
                            return false;
                        }
                        this.publish_statistics_update(host_id, agent, generation, update, cx)
                    })
                    .unwrap_or(false);
                if !active || finished {
                    return;
                }
            }
            // Configuration invalidation drops old senders. Reattach only if this is
            // still the active request, so a closed/replaced page cannot start work.
            let _ = this.update(cx, |this, cx| {
                if this.active_settings().is_some_and(|settings| {
                    settings.navigation.section() == SettingsSection::Statistics
                        && settings.statistics.scan_generation == Some(generation)
                }) {
                    this.load_statistics(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn aggregate_statistics_snapshot(
        &mut self,
        snapshot: Arc<Snapshot>,
        bindings: Arc<[statistics::sources::SourceBinding]>,
        window: DateWindow,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let state = &mut settings.statistics;
        let host = state.host;
        let agent = state.agent;
        let analysis_agent = state.analysis.agent.clone();
        let year = state.analysis.year;
        let shared_revision = state.shared_revision;
        let aggregate_version = state.aggregate_version.clone();
        let version = aggregate_version.fetch_add(1, Ordering::Relaxed) + 1;
        let generation = Uuid::new_v4();
        state.generation = generation;
        state.loading = true;
        state.requested_window = Some(window.clone());
        state.error = None;
        let expected_launch = cx.global::<Config>().agent_launch.clone();
        let expected_host = HostRegistry::lookup(cx, host);
        cx.spawn(async move |this, cx| {
            let presentation = cx
                .background_spawn(async move {
                    cache::compute(move || {
                        if aggregate_version.load(Ordering::Relaxed) != version {
                            return None;
                        }
                        Some(Presentation::from_snapshot(
                            snapshot,
                            bindings,
                            window,
                            analysis_agent.as_deref(),
                            year,
                        ))
                    })
                    .await
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(settings) = this.active_settings_mut() else {
                    return;
                };
                let state = &mut settings.statistics;
                if settings.navigation.section() != SettingsSection::Statistics
                    || state.generation != generation
                    || state.host != host
                    || state.agent != agent
                {
                    return;
                }
                let current_host = HostRegistry::lookup(cx, host);
                let same_host = expected_host
                    .as_ref()
                    .zip(current_host.as_ref())
                    .is_some_and(|(expected, current)| Arc::ptr_eq(expected, current));
                if !same_host
                    || cx.global::<Config>().agent_launch != expected_launch
                    || shared_revision
                        .is_some_and(|revision| !cache::shared().accepts(host, agent, revision))
                {
                    this.load_statistics(cx);
                    return;
                }
                let presentation = match presentation {
                    Ok(Some(presentation)) => presentation,
                    Ok(None) => return,
                    Err(error) => {
                        state.loading = false;
                        state.error =
                            Some(t_fmt(L10nKey::StatisticsReadError, &[("error", &error)]));
                        cx.notify();
                        return;
                    }
                };
                let previous = state.results.insert((host, agent), presentation);
                cx.background_executor()
                    .spawn(async move {
                        drop(previous);
                    })
                    .detach();
                state.loading = state.scan_generation.is_some();
                state.error = None;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn publish_statistics_error(
        &mut self,
        host: HostId,
        agent: Option<&'static str>,
        generation: Uuid,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let state = &mut settings.statistics;
        if state.host != host || state.agent != agent || state.scan_generation != Some(generation) {
            return;
        }
        state.scan_generation = None;
        state.loading = false;
        state.error = Some(t_fmt(L10nKey::StatisticsReadError, &[("error", &error)]));
        cx.notify();
    }
    fn publish_statistics_update(
        &mut self,
        host: HostId,
        agent: Option<&'static str>,
        generation: Uuid,
        update: cache::Update,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(settings) = self.active_settings_mut() else {
            return false;
        };
        let state = &mut settings.statistics;
        if settings.navigation.section() != SettingsSection::Statistics
            || state.host != host
            || state.agent != agent
            || state.scan_generation != Some(generation)
        {
            return false;
        }
        state.management.stored = update.stored;
        state.shared_revision = Some(update.revision);
        if update.finished {
            state.scan_generation = None;
        }
        if let Some(value) = update.value {
            if let Some(window) = state.requested_window.clone() {
                self.aggregate_statistics_snapshot(
                    value.snapshot.clone(),
                    value.bindings.clone(),
                    window,
                    cx,
                );
            }
        } else {
            state.loading = !update.finished;
        }
        if let Some(error) = update.error {
            // Aggregation of an older partial must not erase a terminal error.
            if let Some(settings) = self.active_settings_mut() {
                settings.statistics.generation = Uuid::new_v4();
                settings.statistics.loading = false;
                settings.statistics.error =
                    Some(t_fmt(L10nKey::StatisticsReadError, &[("error", &error)]));
            }
            cx.notify();
        }
        true
    }
    fn select_statistics_host(&mut self, host: HostId, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.statistics.host = host;
            s.statistics.management.reset_scope();
            s.statistics.invalidate();
        }
        self.load_statistics(cx);
    }
    fn select_statistics_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.statistics.period = period;
        }
        self.recompute_statistics(cx);
    }
    fn select_statistics_agent(&mut self, agent: Option<&'static str>, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.statistics.agent = agent;
            settings.statistics.invalidate();
        }
        self.load_statistics(cx);
    }

    pub(crate) fn render_settings_statistics(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(settings) = self.active_settings() else {
            return div().into_any_element();
        };
        let state = &settings.statistics;
        let host = state.host;
        let period = state.period;
        let loading = state.loading;
        let start = state.start.clone();
        let end = state.end.clone();
        let tk = Tk::of(cx);
        let machines = self.agent_hooks_machines(cx);
        let label = machines
            .iter()
            .find(|m| m.host == host)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| format!("{} ({:x})", t(L10nKey::StatisticsOffline), host.0));
        let entries = machines
            .into_iter()
            .map(|m| {
                MenuEntry::item(m.label, m.host == host, move |this, _, cx| {
                    this.select_statistics_host(m.host, cx)
                })
            })
            .collect();
        let machine = self.analysis_menu("statistics-machine", label, entries, cx);
        let selected = state.agent;
        let agents = [
            (None, t(L10nKey::StatisticsAllAgents)),
            (Some("omp"), "omp"),
            (Some("pi"), "pi"),
            (Some("kimi"), "Kimi Code"),
            (Some("opencode"), "OpenCode"),
            (Some("hermes"), "Hermes"),
        ]
        .into_iter()
        .map(|(agent, label)| {
            MenuEntry::item(label, selected == agent, move |this, _, cx| {
                this.select_statistics_agent(agent, cx)
            })
        })
        .collect();
        let agent = self.analysis_menu(
            "statistics-agent",
            selected.map_or(t(L10nKey::StatisticsAllAgents), |agent| match agent {
                "kimi" => "Kimi Code",
                "opencode" => "OpenCode",
                "hermes" => "Hermes",
                other => other,
            }),
            agents,
            cx,
        );
        let periods = [
            Period::Today,
            Period::LastSevenDays,
            Period::Week,
            Period::Month,
            Period::Year,
            Period::All,
            Period::Custom,
        ]
        .into_iter()
        .map(|p| {
            MenuEntry::item(t(period_key(p)), p == period, move |this, _, cx| {
                this.select_statistics_period(p, cx)
            })
        })
        .collect();
        let range = self.analysis_menu("statistics-period", t(period_key(period)), periods, cx);
        let presentation = state.results.get(&(host, state.agent));
        let date_range = presentation
            .map(|p| &p.window)
            .or(state.requested_window.as_ref())
            .map(|window| self.statistics_range_label(window))
            .unwrap_or_else(|| "—".into());
        let toolbar = h_flex()
            .id("statistics-toolbar")
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .id("statistics-date-range")
                            .test_support()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(date_range),
                    )
                    .child(h_flex().flex_wrap().gap_1().child(machine).child(agent)),
            )
            .child(range)
            .child(
                self.analysis_button("statistics-refresh", "", cx, |this, _, cx| {
                    this.refresh_statistics(cx)
                })
                .ghost()
                .icon(gpui_kit::component::IconName::RefreshCw)
                .accessibility_label(t(L10nKey::StatisticsRefresh))
                .disabled(loading || state.management.busy),
            );
        let mut rows = vec![toolbar.into_any_element()];
        if period == Period::Custom {
            rows.push(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Input::new(&start)
                            .id("statistics-start-date")
                            .aria_label(t(L10nKey::StatisticsStartDate))
                            .small()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        Input::new(&end)
                            .id("statistics-end-date")
                            .aria_label(t(L10nKey::StatisticsEndDate))
                            .small()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(self.analysis_button(
                        "statistics-apply",
                        t(L10nKey::StatisticsApply),
                        cx,
                        |this, _, cx| this.recompute_statistics(cx),
                    ))
                    .into_any_element(),
            );
        }
        if loading {
            rows.push(
                h_flex()
                    .id("statistics-loading")
                    .test_support()
                    .role(gpui_kit::Role::Status)
                    .aria_label(t(L10nKey::StatisticsLoading))
                    .gap_2()
                    .text_xs()
                    .text_color(tk.k5)
                    .child(Spinner::new().small().color(tk.k5))
                    .child(t(L10nKey::StatisticsLoading))
                    .into_any_element(),
            );
        }
        if let Some(error) = &state.error {
            rows.push(
                div()
                    .text_sm()
                    .text_color(tk.danger)
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        if let Some(p) = presentation {
            let time = chrono::DateTime::from_timestamp(p.snapshot.updated(), 0)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into());
            let observed = t_fmt(
                if p.totals.is_complete() {
                    L10nKey::StatisticsSnapshotTime
                } else {
                    L10nKey::StatisticsStaleSnapshot
                },
                &[("time", &time)],
            );
            let details = t_fmt(
                L10nKey::StatisticsUpdated,
                &[
                    ("time", &management::format_time(Some(p.snapshot.updated()))),
                    ("zone", p.window.timezone()),
                    ("range", &self.statistics_range_label(&p.window)),
                ],
            );
            rows.push(
                div()
                    .id("statistics-observed")
                    .test_support()
                    .role(gpui_kit::Role::Status)
                    .aria_label(format!("{observed}\n{details}"))
                    .text_xs()
                    .text_color(tk.k5)
                    .child(observed)
                    .into_any_element(),
            );
        }
        rows.push(self.render_statistics_tiles(presentation, cx));
        if let Some(p) = presentation {
            rows.push(self.render_statistics_analysis(p, cx));
        }
        rows.push(self.render_statistics_management_entry(cx));
        let body = div()
            .id("statistics-content")
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&state.content_scroll)
            .child(
                v_flex()
                    .min_w_0()
                    .gap_4()
                    .px_4()
                    .pt_2()
                    .pb_4()
                    .children(rows),
            );
        v_flex()
            .id("statistics-page")
            .size_full()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(crate::ui::scrollbar::with_vertical_scrollbar(
                "statistics-content-scrollbar",
                body,
                &state.content_scroll,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::session::Session;
    use crate::ui::app::LyrelApp;
    use chrono::Datelike;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};

    #[gpui_kit::test]
    fn statistics_search_loads_native_totals_and_keeps_machine_scope(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("sessions")).unwrap();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let sample = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"gui-native\",\"timestamp\":\"{timestamp}\",\"cwd\":\"/work\"}}\n{{\"type\":\"model_usage\",\"id\":\"call\",\"timestamp\":\"{timestamp}\",\"provider\":\"test\",\"model\":\"fixture\",\"usage\":{{\"input\":8,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":10,\"cost\":{{\"total\":0}}}}}}\n"
        );
        std::fs::write(temp.path().join("sessions/native.jsonl"), sample).unwrap();
        let kimi_home = temp.path().join("kimi");
        let kimi_session = kimi_home.join("sessions/wd_test_0123456789ab/session_gui");
        std::fs::create_dir_all(kimi_session.join("agents/main")).unwrap();
        std::fs::write(kimi_session.join("state.json"), serde_json::json!({"id":"session_gui","cwd":"/work","createdAt":chrono::Utc::now().timestamp_millis()}).to_string()).unwrap();
        let mut kimi_wire =
            String::from("{\"type\":\"metadata\",\"protocol_version\":\"1.5\",\"created_at\":1}\n");
        for scope in ["turn", "session"] {
            kimi_wire.push_str(&format!("{}\n", serde_json::json!({"type":"usage.record","agentId":"main","model":"kimi-code/kimi-for-coding","usageScope":scope,"time":chrono::Utc::now().timestamp_millis(),"usage":{"inputOther":3,"output":2,"inputCacheRead":0,"inputCacheCreation":0}})));
        }
        std::fs::write(kimi_session.join("agents/main/wire.jsonl"), kimi_wire).unwrap();
        let pi = temp.path().join("pi-sessions");
        std::fs::create_dir(&pi).unwrap();
        std::fs::write(pi.join("pi.jsonl"), format!("{{\"type\":\"session\",\"version\":3,\"id\":\"gui-native\",\"timestamp\":\"{timestamp}\"}}\n{{\"type\":\"usage\",\"id\":\"call\",\"timestamp\":\"{timestamp}\",\"kind\":\"future\",\"usage\":{{\"input\":5,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{{\"total\":0.25}}}}}}\n")).unwrap();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            let mut config = Config::default();
            config.agent_launch.insert(
                "omp".into(),
                format!(
                    "OMP_PROFILE=default PI_CODING_AGENT_DIR='{}' omp",
                    temp.path().display()
                ),
            );
            config.agent_launch.insert(
                "kimi".into(),
                format!("KIMI_CODE_HOME='{}' kimi", kimi_home.display()),
            );
            config.agent_launch.insert(
                "opencode".into(),
                format!("'{}/missing/opencode'", temp.path().display()),
            );
            config.agent_launch.insert(
                "pi".into(),
                format!(
                    "PI_CODING_AGENT_DIR='{}' pi --session-dir '{}'",
                    temp.path().join("pi-agent").display(),
                    pi.display()
                ),
            );
            config.agent_launch.insert(
                "hermes".into(),
                format!("HERMES_HOME='{}' hermes --safe-mode", temp.path().display()),
            );
            cx.set_global(config);
            crate::ui::keymap::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| LyrelApp::with_session(None, Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        cx.background_executor.run_until_parked();
        let workspace = window
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<LyrelApp>().unwrap()
            })
            .unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::General, window, cx);
            let search = app.active_settings().unwrap().navigation.search().clone();
            search.update(cx, |s, cx| {
                s.set_value(t(L10nKey::StatisticsTitle), window, cx)
            });
            app.autoselect_settings_search(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let waiting =
                vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading);
            if !waiting {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native statistics did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            vcx.run_until_parked();
        }
        app.update_in(&mut vcx, |app, _, cx| {
            app.select_statistics_agent(Some("opencode"), cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            if !vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "OpenCode source failure did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, _, cx| {
            let state = &app.active_settings().unwrap().statistics;
            let failed = state
                .results
                .get(&(HostId::LOCAL, Some("opencode")))
                .expect("diagnostic snapshot");
            assert!(!failed.snapshot.is_complete());
            assert_eq!(failed.totals.tokens().total(), None);
            assert_eq!(
                state
                    .results
                    .get(&(HostId::LOCAL, None))
                    .unwrap()
                    .totals
                    .tokens()
                    .total(),
                Some(27)
            );
            app.select_statistics_agent(None, cx);
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let state = &app.active_settings().unwrap().statistics;
            window.focus(&state.analysis.focus, cx);
            assert!(
                state
                    .results
                    .get(&(HostId::LOCAL, None))
                    .unwrap()
                    .analysis
                    .as_ref()
                    .unwrap()
                    .days()
                    .iter()
                    .any(|d| d.totals().tokens().total() == Some(27))
            );
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("home right");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .active_settings()
                    .unwrap()
                    .statistics
                    .analysis
                    .date
                    .ordinal(),
                8
            );
        });
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let state = &app.read(cx).active_settings().unwrap().statistics.analysis;
            assert_eq!(state.date.ordinal(), 9);
        });
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                !app.read(cx)
                    .active_settings()
                    .unwrap()
                    .statistics
                    .analysis
                    .focus
                    .contains_focused(window, cx),
                "one Tab must leave the annual grid instead of visiting every date"
            );
        });
        vcx.simulate_keystrokes("shift-tab");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert!(
                app.read(cx)
                    .active_settings()
                    .unwrap()
                    .statistics
                    .analysis
                    .focus
                    .is_focused(window),
                "Shift-Tab must return to the single keyboard calendar focus"
            );
        });
        for (locale, width, mode) in [
            ("en", 1120., gpui_kit::component::ThemeMode::Light),
            ("en", 944., gpui_kit::component::ThemeMode::Dark),
            ("zh-CN", 620., gpui_kit::component::ThemeMode::Dark),
        ] {
            crate::ui::i18n::set_locale(locale);
            vcx.simulate_resize(gpui_kit::size(px(width), px(800.)));
            app.update_in(&mut vcx, |app, window, cx| {
                gpui_kit::component::Theme::change(mode, Some(window), cx);
                gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(18.));
                app.active_settings_mut()
                    .unwrap()
                    .statistics
                    .analysis
                    .metric = statistics::analysis::Metric::Tokens;
                cx.notify();
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let top = window.find("statistics-analysis-metric").bounds().top();
                let scroll = &app
                    .read(cx)
                    .active_settings()
                    .unwrap()
                    .statistics
                    .content_scroll;
                let offset = scroll.offset();
                scroll.set_offset(gpui_kit::point(offset.x, offset.y - top + px(180.)));
                window.render_frame(cx);
                let trigger = window.find("statistics-analysis-metric");
                assert!(
                    trigger.visible(),
                    "{locale}: heatmap metric must remain reachable"
                );
                assert!(trigger.bounds().size.width <= px(width));
                window.click("statistics-analysis-metric", cx);
                window.press("escape", cx);
            });
            vcx.run_until_parked();
            app.update_in(&mut vcx, |app, window, cx| {
                window.focus(
                    &app.active_settings().unwrap().statistics.analysis.focus,
                    cx,
                );
            });
            for key in ["end", "home"] {
                vcx.simulate_keystrokes(key);
                vcx.run_until_parked();
                vcx.update(|window, cx| {
                    window.render_frame(cx);
                    let date = app
                        .read(cx)
                        .active_settings()
                        .unwrap()
                        .statistics
                        .analysis
                        .date;
                    let cell = window.find(format!("statistics-day-{date}"));
                    assert!(
                        cell.visible(),
                        "{locale}: {key} must scroll the selected date into view"
                    );
                    let cell = cell.bounds();
                    assert!(
                        (cell.size.width - cell.size.height).abs() <= px(1.),
                        "{locale}: narrow windows must not crush calendar cells"
                    );
                    let weekdays = window.find("statistics-weekdays");
                    let viewport = window.find("statistics-year-weeks").bounds();
                    assert!(
                        weekdays.visible(),
                        "{locale}: weekday labels must remain visible while dates scroll"
                    );
                    assert!(
                        weekdays.bounds().right() <= viewport.left(),
                        "{locale}: weekday labels must stay outside the scrolling date viewport"
                    );
                });
            }
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let days = window.find(("statistics-week", 0usize)).bounds();
                let track = window.find("statistics-calendar-track").bounds();
                assert!(
                    track.top() >= days.bottom(),
                    "{locale}: the horizontal scrollbar must have its own lane below all day rows"
                );
            });
            let mut found = false;
            for _ in 0..450 {
                let focused = vcx.update(|window, cx| {
                    window.render_frame(cx);
                    window.find("statistics-analysis-metric").focused() == Some(true)
                });
                if focused {
                    found = true;
                    break;
                }
                vcx.simulate_keystrokes("tab");
            }
            assert!(
                found,
                "{locale}: metric must be reachable in native Tab order"
            );
            vcx.simulate_keystrokes("enter down down enter");
            vcx.run_until_parked();
            vcx.update(|_, cx| {
                let settings = app.read(cx).active_settings().unwrap();
                assert_eq!(
                    settings.statistics.analysis.metric,
                    statistics::analysis::Metric::Cost,
                    "{locale}: native dropdown must support keyboard selection"
                );
            });
        }
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, _window, cx| {
            let state = app.active_settings().unwrap();
            assert!(state.navigation.section() == SettingsSection::Statistics);
            let totals = &state
                .statistics
                .results
                .get(&(HostId::LOCAL, None))
                .expect("real native snapshot")
                .totals;
            assert_eq!(totals.tokens().total(), Some(27));
            assert_eq!(totals.recorded_usd(), Some(0.25));
            assert_eq!(totals.main_sessions(), 3);
            assert_eq!(totals.unpriced_calls(), 2);
            app.select_statistics_agent(Some("kimi"), cx);
        });
        vcx.run_until_parked();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading) {
            assert!(
                std::time::Instant::now() < deadline,
                "Kimi filter did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            vcx.run_until_parked();
        }
        app.update_in(&mut vcx, |app, _window, cx| {
            let presentation = app
                .active_settings()
                .unwrap()
                .statistics
                .results
                .get(&(HostId::LOCAL, Some("kimi")))
                .unwrap();
            assert_eq!(presentation.totals.tokens().total(), Some(10));
            assert_eq!(presentation.totals.recorded_usd(), None);
            assert_eq!(presentation.totals.main_sessions(), 1);
            assert_eq!(presentation.totals.unpriced_calls(), 2);
            assert!(
                presentation
                    .snapshot
                    .sources()
                    .iter()
                    .all(|source| source.agent() == "kimi")
            );
            app.select_statistics_agent(Some("pi"), cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
            vcx.run_until_parked();
        }
        app.update_in(&mut vcx, |app, _, cx| {
            let totals = &app
                .active_settings()
                .unwrap()
                .statistics
                .results
                .get(&(HostId::LOCAL, Some("pi")))
                .unwrap()
                .totals;
            assert_eq!(totals.tokens().total(), Some(7));
            assert_eq!(totals.recorded_usd(), Some(0.25));
            cx.global_mut::<Config>()
                .agent_launch
                .insert("pi".into(), "PI_CODING_AGENT_DIR=relative pi".into());
            app.select_statistics_agent(None, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
            vcx.run_until_parked();
        }
        app.update_in(&mut vcx, |app, _, _| {
            let presentation = app
                .active_settings()
                .unwrap()
                .statistics
                .results
                .get(&(HostId::LOCAL, None))
                .unwrap();
            assert_eq!(presentation.totals.tokens().total(), Some(20));
            assert!(!presentation.snapshot.is_complete());
            assert!(
                presentation
                    .snapshot
                    .issues()
                    .iter()
                    .any(|issue| issue.path().to_string_lossy() == "pi")
            );
        });
        for agent in [None, Some("hermes")] {
            app.update_in(&mut vcx, |app, _, cx| {
                app.select_statistics_agent(agent, cx)
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                vcx.run_until_parked();
                if !vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            vcx.update(|_, cx| {
                let state = &app.read(cx).active_settings().unwrap().statistics;
                let result = state
                    .results
                    .get(&(HostId::LOCAL, agent))
                    .expect("source result");
                assert_eq!(
                    result.totals.tokens().total(),
                    if agent.is_none() { Some(20) } else { None }
                );
                assert!(!result.snapshot.is_complete());
                if agent == Some("hermes") {
                    assert_eq!(session_count(&result.totals), t(L10nKey::StatisticsUnknown));
                }
            });
        }
        app.update_in(&mut vcx, |app, window, cx| {
            // A disconnected machine must not borrow the local native snapshot.
            app.select_statistics_host(HostId(123456), cx);
            let state = &app.active_settings().unwrap().statistics;
            assert!(state.error.is_some());
            assert!(
                !state
                    .results
                    .contains_key(&(HostId(123456), Some("hermes")))
            );
            app.select_statistics_host(HostId::LOCAL, cx);
            app.navigate_settings(SettingsSection::General, None, window, cx);
            assert!(!app.active_settings().unwrap().statistics.loading);
            app.close_settings(window, cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| assert!(!app.read(cx).has_settings()));
    }
}
