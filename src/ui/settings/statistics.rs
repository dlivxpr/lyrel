//! 所选机器的原生历史总览；所有文件与聚合离开 GPUI 线程。
use super::kit::{Tk, fs};
use super::shell::MenuEntry;
use super::*;
use crate::ui::host_ops::HostOps;
use crate::ui::host_registry::HostRegistry;
use lyrel_core::statistics::{self, DateWindow, Period, Snapshot, StatisticsCache, Totals};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(crate) struct StatisticsState {
    pub(crate) host: HostId,
    agent: &'static str,
    period: Period,
    start: Entity<InputState>,
    end: Entity<InputState>,
    generation: Uuid,
    loading: bool,
    error: Option<String>,
    cache: Arc<Mutex<StatisticsCache>>,
    results: HashMap<(HostId, &'static str), Presentation>,
}
struct Presentation {
    snapshot: Snapshot,
    window: DateWindow,
    totals: Totals,
    previous: Option<Totals>,
}
impl StatisticsState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<LyrelApp>) -> Self {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        Self {
            host: HostId::LOCAL,
            agent: "all",
            period: Period::Month,
            start: cx.new(|cx| InputState::new(window, cx).default_value(date.clone())),
            end: cx.new(|cx| InputState::new(window, cx).default_value(date)),
            generation: Uuid::new_v4(),
            loading: false,
            error: None,
            cache: Arc::new(Mutex::new(StatisticsCache::default())),
            results: HashMap::new(),
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
    let key = if reason.starts_with("Hermes public projects.tree excludes") {
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
        let cache = state.cache.clone();
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
        }
        HostOps::run(
            host,
            cx,
            move |host| -> std::io::Result<Presentation> {
                let mut sources = Vec::new();
                let mut discovery_errors = Vec::new();
                type Discover = fn(
                    &dyn lyrel_core::host::Host,
                    &HashMap<String, String>,
                ) -> std::io::Result<Vec<statistics::Source>>;
                for (slug, discover) in [
                    ("omp", statistics::discover_omp as Discover),
                    ("hermes", statistics::discover_hermes as Discover),
                ] {
                    if agent != "all" && agent != slug {
                        continue;
                    }
                    match discover(host, &launch) {
                        Ok(found) => sources.extend(found),
                        Err(error) => discovery_errors.push((slug, error.to_string())),
                    }
                }
                let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                let mut snapshot = statistics::scan(host, &sources, &mut cache)?;
                for (slug, error) in discovery_errors {
                    snapshot.record_issue(slug.into(), error);
                }
                let totals = snapshot.aggregate(&window);
                let previous = window.previous().map(|w| snapshot.aggregate(w));
                Ok(Presentation {
                    snapshot,
                    window,
                    totals,
                    previous,
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
    fn select_statistics_agent(&mut self, agent: &'static str, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.statistics.agent = agent;
            s.statistics.invalidate();
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
        let agent_entries = [
            ("all", t(L10nKey::StatisticsAllSources).to_string()),
            ("omp", "omp".into()),
            ("hermes", "Hermes".into()),
        ]
        .into_iter()
        .map(|(slug, label)| {
            MenuEntry::item(label, state.agent == slug, move |this, _, cx| {
                this.select_statistics_agent(slug, cx)
            })
        })
        .collect();
        let agent_control = self.settings_dropdown(
            "statistics-agent",
            if state.agent == "all" {
                t(L10nKey::StatisticsAllSources).to_string()
            } else {
                state.agent.to_string()
            },
            None,
            agent_entries,
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
                t(L10nKey::StatisticsRange),
                t(L10nKey::StatisticsCalendar),
                range,
                cx,
            )
            .into_any_element(),
        ];
        rows.push(
            self.settings_row(
                t(L10nKey::StatisticsSource),
                t(L10nKey::StatisticsHermesLimits),
                agent_control,
                cx,
            )
            .into_any_element(),
        );
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
            .and_then(|s| s.statistics.results.get(&(host, s.statistics.agent)))
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
                    L10nKey::StatisticsEstimatedCost,
                    money(totals.estimated_usd()),
                ),
                (L10nKey::StatisticsSessions, session_count(totals)),
                (L10nKey::StatisticsInput, number(totals.tokens().input())),
                (L10nKey::StatisticsOutput, number(totals.tokens().output())),
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
                                ("sessions", &session_count(previous)),
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
                        .child(format!(
                            "{}\n{}",
                            issue.path().display(),
                            issue_text(issue.reason())
                        ))
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
            app.select_statistics_agent("omp", cx);
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
        app.update_in(&mut vcx, |app, _, _| {
            let state = app.active_settings().unwrap();
            assert!(state.section == SettingsSection::Statistics);
            let totals = &state
                .statistics
                .results
                .get(&(HostId::LOCAL, "omp"))
                .expect("real native snapshot")
                .totals;
            assert_eq!(totals.tokens().total(), Some(10));
            assert_eq!(totals.recorded_usd(), Some(0.));
            assert_eq!(totals.main_sessions(), 1);
        });
        // A failing Hermes scope does not erase OMP, and selecting Hermes never borrows OMP totals.
        for agent in ["all", "hermes"] {
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
                    if agent == "all" { Some(10) } else { None }
                );
                assert!(!result.snapshot.is_complete());
                assert!(!result.snapshot.issues().is_empty());
            });
        }
        app.update_in(&mut vcx, |app, window, cx| {
            // A disconnected machine must not borrow the local native snapshot.
            app.select_statistics_host(HostId(123456), cx);
            let state = &app.active_settings().unwrap().statistics;
            assert!(state.error.is_some());
            assert!(!state.results.contains_key(&(HostId(123456), "hermes")));
            app.select_statistics_host(HostId::LOCAL, cx);
            app.navigate_settings(SettingsSection::General, None, window, cx);
            assert!(!app.active_settings().unwrap().statistics.loading);
            app.close_settings(window, cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| assert!(!app.read(cx).has_settings()));
    }
}
