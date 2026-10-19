//! 所选机器的原生历史总览；所有文件与聚合离开 GPUI 线程。
mod analysis;
use super::kit::{Tk, fs};
use super::shell::MenuEntry;
use super::*;
use crate::ui::host_ops::HostOps;
use crate::ui::host_registry::HostRegistry;
use lyrel_core::statistics::{self, DateWindow, Period, Snapshot, StatisticsCache, Totals};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
type Discover = fn(
    &dyn lyrel_core::host::Host,
    &HashMap<String, String>,
) -> std::io::Result<Vec<statistics::Source>>;

pub(crate) struct StatisticsState {
    pub(crate) host: HostId,
    agent: Option<&'static str>,
    period: Period,
    start: Entity<InputState>,
    end: Entity<InputState>,
    generation: Uuid,
    loading: bool,
    error: Option<String>,
    cache: Arc<Mutex<StatisticsCache>>,
    results: HashMap<(HostId, Option<&'static str>), Presentation>,
    analysis: analysis::AnalysisState,
}
struct Presentation {
    snapshot: Snapshot,
    window: DateWindow,
    totals: Totals,
    previous: Option<Totals>,
    analysis: Option<statistics::analysis::Analysis>,
    analysis_error: Option<String>,
}
impl StatisticsState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<LyrelApp>) -> Self {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        Self {
            host: HostId::LOCAL,
            agent: None,
            period: Period::Month,
            start: cx.new(|cx| InputState::new(window, cx).default_value(date.clone())),
            end: cx.new(|cx| InputState::new(window, cx).default_value(date)),
            generation: Uuid::new_v4(),
            loading: false,
            error: None,
            cache: Arc::new(Mutex::new(StatisticsCache::default())),
            results: HashMap::new(),
            analysis: analysis::AnalysisState::new(cx),
        }
    }
    pub(crate) fn invalidate(&mut self) {
        self.generation = Uuid::new_v4();
        self.loading = false;
    }
}
fn period_key(period: Period) -> L10nKey {
    match period {
        Period::Today => L10nKey::StatisticsToday,
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
fn money(value: Option<f64>) -> String {
    value
        .map(|v| format!("USD {v:.4}"))
        .unwrap_or_else(|| t(L10nKey::StatisticsUnpriced).into())
}
impl LyrelApp {
    pub(crate) fn load_statistics(&mut self, cx: &mut Context<Self>) {
        let launch = cx.global::<Config>().agent_launch.clone();
        let Some(settings) = self.active_settings() else {
            return;
        };
        if settings.section != SettingsSection::Statistics {
            return;
        }
        let state = &settings.statistics;
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
                        s.statistics.invalidate();
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
                    s.statistics.invalidate();
                    s.statistics.error = Some(t(L10nKey::StatisticsInvalidDate).into());
                }
                cx.notify();
                return;
            }
        };
        let host_id = state.host;
        let agent = state.agent;
        let mut opencode_sources = Vec::new();
        if agent.is_none_or(|agent| agent == "opencode") {
            use lyrel_core::host::session_operations::{
                SessionOperation, SessionOperationKind, launch_context,
            };
            let native = crate::core::cli_agent::CLIAgent::OpenCode;
            for leaf in self.tabs.iter().flat_map(|tab| tab.pane.terminals()) {
                let pane = leaf.read(cx);
                if pane.host_id() != host_id || pane.agent() != Some(native) {
                    continue;
                }
                let Some(session) = pane.agent_session() else {
                    continue;
                };
                let (Some(scope), Some(cwd), Some(launch_argv)) = (
                    session.readout.session_scope.clone(),
                    pane.host_cwd(),
                    launch_context(native, &session),
                ) else {
                    continue;
                };
                let selector = format!(
                    "opencode:client:{}",
                    scope
                        .opencode
                        .as_ref()
                        .and_then(|context| context.bridge.as_ref())
                        .map_or("disconnected", |bridge| bridge.client_id.as_str())
                );
                if opencode_sources
                    .iter()
                    .any(|source: &statistics::Source| source.scope() == selector)
                {
                    continue;
                }
                opencode_sources.push(statistics::Source::opencode(
                    cwd.clone(),
                    selector,
                    SessionOperation {
                        expected_host: host_id.0,
                        agent: native,
                        kind: SessionOperationKind::Resume,
                        source_id: String::new(),
                        cwd,
                        launch_argv,
                        scope,
                    },
                ));
            }
        }
        let cache = state.cache.clone();
        let analysis_agent = state.analysis.agent.clone();
        let analysis_year = state.analysis.year;
        let analysis_now = chrono::Local::now();
        let Some(host) = HostRegistry::lookup(cx, host_id) else {
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
            s.statistics.loading = true;
            s.statistics.error = None;
            if s.statistics.analysis.anchor.is_none() {
                s.statistics.analysis.anchor =
                    Some(gpui_kit::ScrollAnchor::for_handle(s.content_scroll.clone()));
            }
        }
        HostOps::run(
            host,
            cx,
            move |host| -> std::io::Result<Presentation> {
                let mut sources = Vec::new();
                let mut discovery_errors = Vec::new();
                for (name, discover) in [
                    ("omp", statistics::discover_omp as Discover),
                    ("pi", statistics::discover_pi as Discover),
                    ("kimi", statistics::discover_kimi as Discover),
                ] {
                    if agent.is_some_and(|selected| selected != name) {
                        continue;
                    }
                    match discover(host, &launch) {
                        Ok(found) => sources.extend(found),
                        Err(error) => discovery_errors.push((name, error.to_string())),
                    }
                }
                if agent.is_none_or(|agent| agent == "opencode") {
                    // An explicitly bound client is never replaced by another default service.
                    if opencode_sources.is_empty() {
                        match statistics::discover_opencode(host, &launch) {
                            Ok(found) => sources.extend(found),
                            Err(error) => discovery_errors.push(("OpenCode", error.to_string())),
                        }
                    } else {
                        sources.extend(opencode_sources);
                    }
                }
                let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                let mut snapshot = statistics::scan(host, &sources, &mut cache)?;
                for (name, error) in discovery_errors {
                    snapshot = snapshot.with_issue(name.into(), error);
                }
                let totals = snapshot.aggregate(&window);
                let previous = window.previous().map(|w| snapshot.aggregate(w));
                let analysis = snapshot.analyze(
                    &window,
                    analysis_now,
                    analysis_agent.as_deref(),
                    analysis_year,
                );
                let (analysis, analysis_error) = match analysis {
                    Ok(value) => (Some(value), None),
                    Err(error) => (None, Some(error)),
                };
                Ok(Presentation {
                    snapshot,
                    window,
                    totals,
                    previous,
                    analysis,
                    analysis_error,
                })
            },
            move |this, result, cx| {
                let Some(settings) = this.active_settings_mut() else {
                    return;
                };
                let state = &mut settings.statistics;
                if settings.section != SettingsSection::Statistics
                    || state.generation != generation
                    || state.host != host_id
                    || state.agent != agent
                {
                    return;
                }
                state.loading = false;
                match result {
                    Ok(presentation) => {
                        state.results.insert((host_id, agent), presentation);
                        state.error = None;
                    }
                    Err(error) => {
                        state.error = Some(t_fmt(
                            L10nKey::StatisticsReadError,
                            &[("error", &error.to_string())],
                        ));
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    fn select_statistics_host(&mut self, host: HostId, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.statistics.host = host;
            s.statistics.invalidate();
        }
        self.load_statistics(cx);
    }
    fn select_statistics_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.statistics.period = period;
            s.statistics.invalidate();
        }
        self.load_statistics(cx);
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
        let machine = self.settings_dropdown("statistics-machine", label, None, entries, cx);
        let selected = state.agent;
        let agents = [
            (None, t(L10nKey::StatisticsAllAgents)),
            (Some("omp"), "omp"),
            (Some("pi"), "pi"),
            (Some("kimi"), "Kimi Code"),
            (Some("opencode"), "OpenCode"),
        ]
        .into_iter()
        .map(|(agent, label)| {
            MenuEntry::item(label, selected == agent, move |this, _, cx| {
                this.select_statistics_agent(agent, cx)
            })
        })
        .collect();
        let agent = self.settings_dropdown(
            "statistics-agent",
            selected.map_or(t(L10nKey::StatisticsAllAgents), |agent| match agent {
                "kimi" => "Kimi Code",
                "opencode" => "OpenCode",
                other => other,
            }),
            None,
            agents,
            cx,
        );
        let periods = [
            Period::Today,
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
        let range = self.settings_dropdown(
            "statistics-period",
            t(period_key(period)),
            None,
            periods,
            cx,
        );
        let mut rows = vec![
            self.settings_row(
                t(L10nKey::StatisticsTitle),
                t(L10nKey::StatisticsScope),
                machine,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(L10nKey::StatisticsAgent),
                t(L10nKey::StatisticsAgentScope),
                agent,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(L10nKey::StatisticsRange),
                t(L10nKey::StatisticsCalendar),
                range,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(L10nKey::StatisticsAgent),
                t(L10nKey::StatisticsAgentScope),
                agent,
                cx,
            )
            .into_any_element(),
        ];
        if period == Period::Custom {
            rows.push(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(fs(12.))
                            .child(t(L10nKey::StatisticsCustomDates)),
                    )
                    .child(Input::new(&start).w_full())
                    .child(Input::new(&end).w_full())
                    .child(self.settings_button(
                        "statistics-apply",
                        t(L10nKey::StatisticsApply),
                        cx,
                        |this, _, cx| this.load_statistics(cx),
                    ))
                    .into_any_element(),
            );
        }
        rows.push(
            self.settings_button(
                "statistics-refresh",
                t(L10nKey::StatisticsRefresh),
                cx,
                |this, _, cx| this.load_statistics(cx),
            )
            .disabled(loading)
            .into_any_element(),
        );
        if loading {
            rows.push(
                div()
                    .text_color(tk.k5)
                    .child(t(L10nKey::StatisticsLoading))
                    .into_any_element(),
            );
        }
        if let Some(error) = self
            .active_settings()
            .and_then(|s| s.statistics.error.clone())
        {
            rows.push(div().text_color(tk.danger).child(error).into_any_element());
        }
        if let Some(p) = self
            .active_settings()
            .and_then(|s| s.statistics.results.get(&(host, state.agent)))
        {
            let totals = &p.totals;
            let range_label = match (p.window.start(), p.window.end()) {
                (Some(start), Some(end)) => format!(
                    "{} — {}",
                    chrono::DateTime::from_timestamp(start, 0)
                        .unwrap()
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d"),
                    chrono::DateTime::from_timestamp(end - 1, 0)
                        .unwrap()
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                ),
                _ => t(L10nKey::StatisticsAll).into(),
            };
            rows.push(
                div()
                    .text_color(tk.k5)
                    .child(t_fmt(
                        L10nKey::StatisticsUpdated,
                        &[
                            (
                                "time",
                                &chrono::DateTime::from_timestamp(p.snapshot.updated(), 0)
                                    .unwrap()
                                    .with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d %H:%M:%S %:z")
                                    .to_string(),
                            ),
                            ("zone", p.window.timezone()),
                            ("range", &range_label),
                        ],
                    ))
                    .into_any_element(),
            );
            rows.push(
                div()
                    .text_color(if totals.is_complete() {
                        tk.k5
                    } else {
                        tk.warn_text
                    })
                    .child(t(if totals.is_complete() {
                        L10nKey::StatisticsComplete
                    } else {
                        L10nKey::StatisticsPartial
                    }))
                    .into_any_element(),
            );
            let values = [
                (L10nKey::StatisticsTokens, number(totals.tokens().total())),
                (L10nKey::StatisticsCost, money(totals.recorded_usd())),
                (
                    L10nKey::StatisticsSessions,
                    number(Some(totals.main_sessions())),
                ),
                (L10nKey::StatisticsInput, number(totals.tokens().input())),
                (L10nKey::StatisticsOutput, number(totals.tokens().output())),
                (
                    L10nKey::StatisticsReasoning,
                    number(totals.tokens().reasoning()),
                ),
                (
                    L10nKey::StatisticsCacheRead,
                    number(totals.tokens().cache_read()),
                ),
                (
                    L10nKey::StatisticsCacheWrite,
                    number(totals.tokens().cache_write()),
                ),
                (
                    L10nKey::StatisticsAverage,
                    totals
                        .average_tokens()
                        .map(|v| format!("{v:.1}"))
                        .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                ),
            ];
            for (key, value) in values {
                rows.push(
                    h_flex()
                        .flex_wrap()
                        .gap_3()
                        .justify_between()
                        .child(t(key))
                        .child(div().font_weight(FontWeight::MEDIUM).child(value))
                        .into_any_element(),
                );
            }
            rows.push(
                div()
                    .text_color(tk.k5)
                    .child(t_fmt(
                        L10nKey::StatisticsUnpricedCalls,
                        &[("count", &totals.unpriced_calls().to_string())],
                    ))
                    .into_any_element(),
            );
            if let Some(previous) = &p.previous {
                let change = totals
                    .token_change(previous)
                    .map(|v| format!("{v:+.1}%"))
                    .unwrap_or_else(|| t(L10nKey::StatisticsNotComparable).into());
                rows.push(
                    div()
                        .text_color(tk.k5)
                        .child(t_fmt(
                            L10nKey::StatisticsComparison,
                            &[
                                ("tokens", &number(previous.tokens().total())),
                                ("cost", &money(previous.recorded_usd())),
                                ("sessions", &previous.main_sessions().to_string()),
                                ("change", &change),
                            ],
                        ))
                        .into_any_element(),
                );
            }
            if totals.calls() == 0 {
                rows.push(
                    div()
                        .text_color(tk.k5)
                        .child(t(L10nKey::StatisticsEmpty))
                        .into_any_element(),
                );
            }
            rows.push(self.render_statistics_analysis(p, cx));
            for source in p.snapshot.sources() {
                rows.push(
                    div()
                        .text_size(fs(12.))
                        .text_color(tk.k5)
                        .child(format!(
                            "{} · {}\n{}",
                            source.agent(),
                            source.scope(),
                            source.root().display()
                        ))
                        .into_any_element(),
                );
            }
            for issue in p.snapshot.issues() {
                rows.push(
                    div()
                        .text_size(fs(12.))
                        .text_color(tk.warn_text)
                        .child(format!("{}\n{}", issue.path().display(), issue.reason()))
                        .into_any_element(),
                );
            }
        }
        rows.push(
            div()
                .text_size(fs(12.))
                .text_color(tk.k5)
                .child(t(L10nKey::StatisticsPrivacy))
                .into_any_element(),
        );
        Self::settings_page([v_flex().gap_4().children(rows).into_any_element()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::session::Session;
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
            cx.set_global(config);
            crate::ui::keymap::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| LyrelApp::with_session(None, Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        cx.background_executor.run_until_parked();
        let app = window
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<LyrelApp>().unwrap()
            })
            .unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::General, window, cx);
            let search = app.active_settings().unwrap().search.clone();
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
        vcx.simulate_keystrokes("home right down enter");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let state = &app.read(cx).active_settings().unwrap().statistics.analysis;
            assert_eq!(state.date.ordinal(), 9);
            assert!(state.detail);
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let settings = app.read(cx).active_settings().unwrap();
            assert!(!settings.statistics.analysis.detail);
            assert!(settings.section == SettingsSection::Statistics);
        });
        for (locale, width, mode) in [
            ("en", 1120., gpui_kit::component::ThemeMode::Light),
            ("zh-CN", 620., gpui_kit::component::ThemeMode::Dark),
        ] {
            crate::ui::i18n::set_locale(locale);
            vcx.simulate_window_resize(window.into(), gpui_kit::size(px(width), px(800.)));
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
                let scroll = &app.read(cx).active_settings().unwrap().content_scroll;
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
        app.update_in(&mut vcx, |app, window, cx| {
            let state = app.active_settings().unwrap();
            assert!(state.section == SettingsSection::Statistics);
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
            let totals = &app.active_settings().unwrap().statistics.results
                .get(&(HostId::LOCAL, Some("pi"))).unwrap().totals;
            assert_eq!(totals.tokens().total(), Some(7));
            assert_eq!(totals.recorded_usd(), Some(0.25));
            cx.global_mut::<Config>().agent_launch.insert(
                "pi".into(), "PI_CODING_AGENT_DIR=relative pi".into());
            app.select_statistics_agent(None, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while vcx.update(|_, cx| app.read(cx).active_settings().unwrap().statistics.loading) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
            vcx.run_until_parked();
        }
        app.update_in(&mut vcx, |app, window, cx| {
            let presentation = app.active_settings().unwrap().statistics.results
                .get(&(HostId::LOCAL, None)).unwrap();
            assert_eq!(presentation.totals.tokens().total(), Some(20));
            assert!(!presentation.snapshot.is_complete());
            assert!(presentation.snapshot.issues().iter().any(|issue| issue.path().to_string_lossy() == "pi"));
            // A disconnected machine must not borrow the local native snapshot.
            app.select_statistics_host(HostId(123456), cx);
            let state = &app.active_settings().unwrap().statistics;
            assert!(state.error.is_some());
            assert!(!state.results.contains_key(&(HostId(123456), None)));
            app.select_statistics_host(HostId::LOCAL, cx);
            app.navigate_settings(SettingsSection::General, None, window, cx);
            assert!(!app.active_settings().unwrap().statistics.loading);
            app.close_settings(window, cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| assert!(!app.read(cx).has_settings()));
    }
}
