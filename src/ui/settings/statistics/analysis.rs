//! 统计页内分析交互；不拥有第二份来源、缓存或聚合算法。
use super::*;
use chrono::{Datelike, Days, Months, NaiveDate};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    button::Button,
    menu::{DropdownMenu as _, PopupMenuItem},
    scroll::Scrollbar,
};
use gpui_kit::{FocusHandle, rems};
use lyrel_core::statistics::analysis::{Analysis, Breakdown, Metric};

pub(super) struct AnalysisState {
    pub(super) agent: Option<String>,
    pub(super) year: i32,
    pub(super) date: NaiveDate,
    pub(super) summary_detail: bool,
    pub(super) focus: FocusHandle,
    pub(super) anchor: Option<gpui_kit::ScrollAnchor>,
    calendar_scroll: gpui_kit::ScrollHandle,
    pub(super) metric: Metric,
    group: GroupKind,
    page: usize,
    expanded: Option<String>,
    agent_page: usize,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Models,
    Projects,
}
impl AnalysisState {
    pub(super) fn new(cx: &mut Context<LyrelApp>) -> Self {
        let date = chrono::Local::now().date_naive();
        let state = Self {
            agent: None,
            year: date.year(),
            date,
            summary_detail: false,
            focus: cx.focus_handle().tab_stop(true),
            anchor: None,
            calendar_scroll: gpui_kit::ScrollHandle::new(),
            metric: Metric::Tokens,
            group: GroupKind::Models,
            page: 0,
            expanded: None,
            agent_page: 0,
        };
        state.reveal_date();
        state
    }
    fn reveal_date(&self) {
        let offset = NaiveDate::from_ymd_opt(self.year, 1, 1)
            .unwrap()
            .weekday()
            .num_days_from_monday();
        self.calendar_scroll
            .scroll_to_item(((offset + self.date.ordinal0()) / 7) as usize);
    }
}
fn agent_label(agent: &str) -> &str {
    match agent {
        "kimi" => "Kimi Code",
        "opencode" => "OpenCode",
        "hermes" => "Hermes",
        other => other,
    }
}
fn agent_opacity(agent: &str) -> f32 {
    match agent {
        "omp" => 1.,
        "pi" => 0.8,
        "opencode" => 0.65,
        "hermes" => 0.5,
        _ => 0.35,
    }
}

fn metric_key(metric: Metric) -> L10nKey {
    match metric {
        Metric::Tokens => L10nKey::StatisticsTokens,
        Metric::Cost => L10nKey::StatisticsPricedCost,
        Metric::Sessions => L10nKey::StatisticsSessions,
    }
}
fn group_key(group: GroupKind) -> L10nKey {
    match group {
        GroupKind::Models => L10nKey::StatisticsModels,
        GroupKind::Projects => L10nKey::StatisticsProjects,
    }
}

// Rows show observed values even when another source limits coverage. These are not
// comparable complete-period values and must not feed heatmap intensity or deltas.
fn breakdown_value(totals: &Totals, metric: Metric) -> Option<f64> {
    match metric {
        Metric::Tokens => totals.tokens().total().map(|value| value as f64),
        Metric::Cost => metric.value(totals).or_else(|| {
            if totals.unpriced_calls() > 0
                || totals
                    .estimated_costs()
                    .iter()
                    .any(|(currency, estimate)| currency != "USD" || !estimate.is_complete())
            {
                return None;
            }
            let recorded = totals.recorded_usd();
            let estimated = totals
                .estimated_costs()
                .get("USD")
                .and_then(|estimate| estimate.total());
            match (recorded, estimated) {
                (None, None) => None,
                (recorded, estimated) => Some(recorded.unwrap_or(0.) + estimated.unwrap_or(0.)),
            }
        }),
        Metric::Sessions => {
            (totals.main_sessions() > 0 || totals.calls() > 0 || totals.is_complete())
                .then_some(totals.main_sessions() as f64)
        }
    }
}
pub(super) fn activity_hover(day: &lyrel_core::statistics::analysis::Day) -> String {
    let value = |metric| {
        let Some(value) = day.activity_value(metric) else {
            return "—".to_owned();
        };
        match metric {
            Metric::Cost => format!("${value:.2}"),
            Metric::Tokens => {
                let precision = if value >= 1_000_000. { 2 } else { 6 };
                let mut millions = format!("{:.*}", precision, value / 1_000_000.);
                millions.truncate(millions.trim_end_matches('0').trim_end_matches('.').len());
                millions.push('M');
                millions
            }
            Metric::Sessions => number(Some(value as u64)),
        }
    };
    t_fmt(
        L10nKey::StatisticsActivityValues,
        &[
            ("tokens", &value(Metric::Tokens)),
            ("cost", &value(Metric::Cost)),
            ("sessions", &value(Metric::Sessions)),
        ],
    )
}

pub(super) fn detail(totals: &Totals) -> String {
    let empty = totals.calls() == 0 && totals.is_complete();
    let tokens = |value: Option<u64>| number(value.or_else(|| empty.then_some(0)));
    let native = t_fmt(
        L10nKey::StatisticsAnalysisValues,
        &[
            ("tokens", &tokens(totals.tokens().total())),
            (
                "cost",
                &money(totals.recorded_usd().or_else(|| empty.then_some(0.))),
            ),
            (
                "estimated",
                &totals
                    .estimated_usd()
                    .map(|v| format!("USD {v:.4}"))
                    .unwrap_or_else(|| "—".into()),
            ),
            ("sessions", &session_count(totals)),
            ("calls", &totals.calls().to_string()),
            ("cumulative", &totals.cumulative_records().to_string()),
            ("unpriced", &totals.unpriced_calls().to_string()),
            ("reasoning", &tokens(totals.tokens().reasoning())),
            ("input", &tokens(totals.tokens().input())),
            ("output", &tokens(totals.tokens().output())),
            ("read", &tokens(totals.tokens().cache_read())),
            ("write", &tokens(totals.tokens().cache_write())),
            (
                "status",
                t(if totals.is_complete() {
                    L10nKey::StatisticsComplete
                } else {
                    L10nKey::StatisticsPartial
                }),
            ),
        ],
    );
    let estimates = totals
        .estimated_costs()
        .iter()
        .map(|(currency, estimate)| {
            t_fmt(
                L10nKey::StatisticsPriceEstimate,
                &[
                    ("currency", currency),
                    (
                        "custom",
                        &estimate
                            .custom
                            .map(|v| format!("{v:.6}"))
                            .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                    ),
                    (
                        "catalog",
                        &estimate
                            .catalog
                            .map(|v| format!("{v:.6}"))
                            .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                    ),
                    ("priced", &estimate.priced_tokens.to_string()),
                    ("unpriced", &estimate.unpriced_tokens.to_string()),
                    ("partial", &estimate.partial_calls.to_string()),
                ],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if estimates.is_empty() {
        native
    } else {
        format!("{native}\n{estimates}")
    }
}
impl LyrelApp {
    pub(in crate::ui::settings) fn analysis_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Button {
        Button::new(id)
            .label(label)
            .small()
            .ghost()
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
    }
    pub(in crate::ui::settings) fn analysis_menu(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        entries: Vec<MenuEntry>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = cx.entity().downgrade();
        Button::new(id)
            .label(label)
            .small()
            .ghost()
            .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |mut menu, _, _| {
                for entry in &entries {
                    menu = match entry {
                        MenuEntry::Item {
                            label,
                            checked,
                            on_pick,
                            ..
                        } => {
                            let app = app.clone();
                            let on_pick = on_pick.clone();
                            menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(*checked)
                                    .on_click(move |_, window, cx| {
                                        let _ = app.update(cx, |app, cx| on_pick(app, window, cx));
                                    }),
                            )
                        }
                        MenuEntry::Separator => menu.separator(),
                        MenuEntry::Heading(label) | MenuEntry::Note(label) => {
                            menu.item(PopupMenuItem::label(label.clone()))
                        }
                    };
                }
                menu
            })
            .into_any_element()
    }
    pub(super) fn change_analysis(
        &mut self,
        change: impl FnOnce(&mut AnalysisState),
        reload: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.active_settings_mut() {
            change(&mut s.statistics.analysis);
            s.statistics.analysis.reveal_date();
        }
        if reload {
            self.recompute_statistics(cx);
        }
        cx.notify();
    }
    fn select_analysis_date(
        &mut self,
        date: NaiveDate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if date > chrono::Local::now().date_naive() {
            return;
        }
        if let Some(s) = self.active_settings_mut() {
            s.statistics.analysis.date = date;
            s.statistics.analysis.reveal_date();
            window.focus(&s.statistics.analysis.focus, cx);
            if let Some(anchor) = &s.statistics.analysis.anchor {
                anchor.scroll_to(window, cx);
            }
        }
        cx.notify();
    }
    fn analysis_calendar_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let state = &mut settings.statistics.analysis;
        let date = state.date;
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.secondary()
        {
            return;
        }
        let moved = match key {
            "left" => date.checked_sub_days(Days::new(7)),
            "right" => date.checked_add_days(Days::new(7)),
            "up" => date.checked_sub_days(Days::new(1)),
            "down" => date.checked_add_days(Days::new(1)),
            "pageup" => date.checked_sub_months(Months::new(1)),
            "pagedown" => date.checked_add_months(Months::new(1)),
            "home" => NaiveDate::from_ymd_opt(state.year, 1, 1),
            "end" => NaiveDate::from_ymd_opt(state.year, 12, 31)
                .map(|end| end.min(chrono::Local::now().date_naive())),
            _ => return,
        };
        if let Some(date) =
            moved.filter(|d| d.year() == state.year && *d <= chrono::Local::now().date_naive())
        {
            state.date = date;
        }
        state.reveal_date();
        if let Some(anchor) = &state.anchor {
            anchor.scroll_to(window, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }
    // Keep each region's builder temporaries off the same Windows Debug stack frame.
    pub(super) fn render_statistics_analysis(
        &self,
        p: &Presentation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let activity = self.render_statistics_activity(p, cx);
        let Some(analysis) = &p.analysis else {
            return activity;
        };
        v_flex()
            .min_w_0()
            .gap_3()
            .child(activity)
            .when(!analysis.totals().is_complete(), |section| {
                section.child(
                    div()
                        .text_xs()
                        .text_color(Tk::of(cx).k5)
                        .child(t(L10nKey::StatisticsObservedValues)),
                )
            })
            .child(
                h_flex()
                    .items_stretch()
                    .flex_wrap()
                    .gap_3()
                    .child(self.render_statistics_agents(analysis, cx))
                    .child(self.render_statistics_common(analysis, cx)),
            )
            .into_any_element()
    }

    fn render_statistics_activity(&self, p: &Presentation, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.active_settings().unwrap().statistics.analysis;
        let tk = Tk::of(cx);
        let accent = cx.theme().primary;
        let today = chrono::Local::now().date_naive();
        let mut agents: Vec<_> = p
            .snapshot
            .sources()
            .iter()
            .map(|s| s.agent().to_owned())
            .collect();
        agents.sort();
        agents.dedup();
        let mut entries = vec![MenuEntry::item(
            t(L10nKey::StatisticsAllAgents),
            state.agent.is_none(),
            |this, _, cx| {
                this.change_analysis(
                    |s| {
                        s.agent = None;
                        s.page = 0;
                    },
                    true,
                    cx,
                )
            },
        )];
        entries.extend(agents.into_iter().map(|agent| {
            MenuEntry::item(
                agent_label(&agent).to_owned(),
                state.agent.as_deref() == Some(&agent),
                move |this, _, cx| {
                    this.change_analysis(
                        |s| {
                            s.agent = Some(agent.clone());
                            s.page = 0;
                        },
                        true,
                        cx,
                    )
                },
            )
        }));
        let agent = self.analysis_menu(
            "statistics-analysis-agent",
            state
                .agent
                .as_deref()
                .map(agent_label)
                .unwrap_or(t(L10nKey::StatisticsAllAgents)),
            entries,
            cx,
        );
        let metric = self.analysis_menu(
            "statistics-analysis-metric",
            t(metric_key(state.metric)),
            [Metric::Tokens, Metric::Cost, Metric::Sessions]
                .into_iter()
                .map(|metric| {
                    MenuEntry::item(
                        t(metric_key(metric)),
                        state.metric == metric,
                        move |this, _, cx| this.change_analysis(|s| s.metric = metric, false, cx),
                    )
                })
                .collect(),
            cx,
        );
        let year = state.year;
        let year_controls = h_flex()
            .gap_1()
            .child(
                self.analysis_button("statistics-year-previous", "‹", cx, move |this, _, cx| {
                    this.change_analysis(
                        |s| {
                            s.year = year - 1;
                            s.date = NaiveDate::from_ymd_opt(s.year, 1, 1).unwrap();
                        },
                        true,
                        cx,
                    )
                })
                .accessibility_label(t(L10nKey::StatisticsPreviousYear))
                .disabled(year <= 1),
            )
            .child(div().text_sm().child(year.to_string()))
            .child(
                self.analysis_button("statistics-year-next", "›", cx, move |this, _, cx| {
                    this.change_analysis(
                        |s| {
                            s.year = year + 1;
                            s.date = NaiveDate::from_ymd_opt(s.year, 1, 1).unwrap();
                        },
                        true,
                        cx,
                    )
                })
                .accessibility_label(t(L10nKey::StatisticsNextYear))
                .disabled(year >= today.year()),
            );
        let mut activity = v_flex()
            .id("statistics-activity")
            .min_w_0()
            .w_full()
            .gap_2()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t(L10nKey::StatisticsActivity)),
                            )
                            .child(div().text_xs().text_color(tk.k5).child(t_fmt(
                                L10nKey::StatisticsActivityCaption,
                                &[("metric", t(metric_key(state.metric)))],
                            ))),
                    )
                    .child(metric),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .gap_2()
                    .child(agent)
                    .child(year_controls)
                    .when(
                        p.analysis.as_ref().and_then(|a| a.agent_filter())
                            != state.agent.as_deref(),
                        |row| {
                            row.child(
                                div().text_xs().text_color(tk.k5).child(t_fmt(
                                    L10nKey::StatisticsAnalysisSnapshot,
                                    &[(
                                        "agent",
                                        p.analysis
                                            .as_ref()
                                            .and_then(|a| a.agent_filter())
                                            .unwrap_or(t(L10nKey::StatisticsAllAgents)),
                                    )],
                                )),
                            )
                        },
                    ),
            );
        if let Some(error) = &p.analysis_error {
            activity = activity.child(
                div()
                    .text_sm()
                    .text_color(tk.danger)
                    .child(t_fmt(L10nKey::StatisticsAnalysisError, &[("error", error)])),
            );
        }
        let Some(analysis) = &p.analysis else {
            return activity.into_any_element();
        };
        if !analysis.has_sources() {
            activity = activity.child(
                div()
                    .text_xs()
                    .text_color(tk.k5)
                    .child(t(L10nKey::StatisticsNoAnalysisSources)),
            );
        }
        let grid = self.render_statistics_calendar(analysis, today, cx);
        activity = activity.child(grid).child(
            h_flex()
                .justify_end()
                .min_w_0()
                .flex_wrap()
                .gap_2()
                .text_xs()
                .text_color(tk.k5)
                .child(
                    h_flex()
                        .gap_1()
                        .child(t(L10nKey::StatisticsLess))
                        .children([0., 0.25, 0.5, 0.75, 1.].map(|level| {
                            div().size_2().rounded_sm().bg(if level == 0. {
                                tk.k06
                            } else {
                                accent.opacity(level)
                            })
                        }))
                        .child(t(L10nKey::StatisticsMore)),
                ),
        );
        activity.into_any_element()
    }

    fn render_statistics_calendar(
        &self,
        analysis: &Analysis,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.active_settings().unwrap().statistics.analysis;
        let tk = Tk::of(cx);
        let accent = cx.theme().primary;
        let maximum = analysis
            .days()
            .iter()
            .filter(|day| day.date() <= today)
            .filter_map(|day| day.activity_value(state.metric))
            .fold(0., f64::max);
        let offset = analysis
            .days()
            .first()
            .map_or(0, |d| d.date().weekday().num_days_from_monday() as usize);
        let weeks = (offset + analysis.days().len()).div_ceil(7);
        let columns = (0..weeks)
            .map(|week| {
                let first = (week * 7).saturating_sub(offset);
                let month = analysis
                    .days()
                    .get(first)
                    .filter(|day| week == 0 || (day.date().month() != 1 && day.date().day() <= 7))
                    .map(|day| day.date().format("%m").to_string())
                    .unwrap_or_default();
                // Keep square days at the UI scale; narrow windows scroll rather than crush them.
                v_flex()
                    .id(("statistics-week", week))
                    .test_support()
                    .flex_none()
                    .w_3()
                    .gap_0p5()
                    .child(
                        div()
                            .h_4()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(tk.k5)
                            .child(month),
                    )
                    .children((0..7).map(|weekday| {
                        let day = (week * 7 + weekday)
                            .checked_sub(offset)
                            .and_then(|ix| analysis.days().get(ix));
                        let Some(day) = day else {
                            return div().h_3().w_full().into_any_element();
                        };
                        let date = day.date();
                        if date > today {
                            return div().h_3().w_full().into_any_element();
                        }
                        let value = day.activity_value(state.metric);
                        let background = match value {
                            Some(v) if v > 0. && maximum > 0. => {
                                cx.theme().primary.opacity(0.2 + 0.8 * (v / maximum) as f32)
                            }
                            _ => tk.k06,
                        };
                        let description = activity_hover(day);
                        let name = format!("{date}\n{description}");
                        Button::new(SharedString::from(format!("statistics-day-{date}")))
                            .ghost()
                            .xsmall()
                            .tab_stop(false)
                            .h_3()
                            .w_full()
                            .min_w_0()
                            .p_0()
                            .rounded_sm()
                            .bg(background)
                            .border_1()
                            .border_color(if date == state.date {
                                cx.theme().primary
                            } else if day.is_in_range()
                                && (!day.totals().is_complete() || value.is_none())
                            {
                                cx.theme().border
                            } else {
                                background
                            })
                            .when(!day.is_in_range(), |b| b.opacity(0.35))
                            .accessibility_label(name)
                            .tooltip(description)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_analysis_date(date, window, cx)
                            }))
                            .into_any_element()
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let weekdays = v_flex()
            .id("statistics-weekdays")
            .test_support()
            .flex_none()
            .whitespace_nowrap()
            .gap_0p5()
            .child(div().h_4())
            .children(
                t(L10nKey::StatisticsWeekdays)
                    .split_whitespace()
                    .map(|label| {
                        div()
                            .h_3()
                            .line_height(rems(0.75))
                            .text_xs()
                            .text_color(tk.k5)
                            .child(label.to_owned())
                    }),
            );
        let grid = v_flex()
            .id("statistics-calendar")
            .relative()
            .anchor_scroll(state.anchor.clone())
            .track_focus(&state.focus)
            .role(gpui_kit::Role::Group)
            .aria_label(t(L10nKey::StatisticsCalendarKeys))
            .min_w_0()
            .gap_2()
            .flex_shrink_0()
            .rounded_md()
            .border_1()
            .border_color(tk.k06)
            .p_1()
            .focus_visible(move |s| s.border_color(accent))
            .on_key_down(cx.listener(Self::analysis_calendar_key))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_0p5()
                    .child(weekdays)
                    .child(
                        v_flex()
                            .w_full()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .id("statistics-year-weeks")
                                    .test_support()
                                    .w_full()
                                    .min_w_0()
                                    .items_start()
                                    .overflow_x_scroll()
                                    .track_scroll(&state.calendar_scroll)
                                    .gap_0p5()
                                    .children(columns),
                            )
                            .child(
                                div()
                                    .id("statistics-calendar-track")
                                    .test_support()
                                    .relative()
                                    .w_full()
                                    // The framework's physical hit lane sits below, not over, the days.
                                    .h(Scrollbar::width())
                                    .child(
                                        Scrollbar::horizontal(&state.calendar_scroll)
                                            .viewport_from_layout(),
                                    ),
                            ),
                    ),
            );
        grid.into_any_element()
    }

    fn render_statistics_common(&self, analysis: &Analysis, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.active_settings().unwrap().statistics.analysis;
        let tk = Tk::of(cx);
        let groups_menu = self.analysis_menu(
            "statistics-analysis-group",
            t(group_key(state.group)),
            [GroupKind::Models, GroupKind::Projects]
                .into_iter()
                .map(|group| {
                    MenuEntry::item(
                        t(group_key(group)),
                        state.group == group,
                        move |this, _, cx| {
                            this.change_analysis(
                                |s| {
                                    s.group = group;
                                    s.page = 0;
                                },
                                false,
                                cx,
                            )
                        },
                    )
                })
                .collect(),
            cx,
        );
        let mut groups: Vec<_> = match state.group {
            GroupKind::Models => analysis.models(),
            GroupKind::Projects => analysis.projects(),
        }
        .iter()
        .collect();
        groups.sort_by(|a, b| {
            match (
                breakdown_value(a.totals(), state.metric),
                breakdown_value(b.totals(), state.metric),
            ) {
                (Some(a), Some(b)) => b.total_cmp(&a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        let maximum = groups
            .iter()
            .filter_map(|group| breakdown_value(group.totals(), state.metric))
            .fold(0., f64::max);
        let pages = groups.len().div_ceil(10).max(1);
        let page = state.page.min(pages - 1);
        let common =
            v_flex()
                .flex_grow(1.)
                .flex_shrink(1.)
                .flex_basis(rems(17.5))
                .min_w_0()
                .gap_2()
                .py_3()
                .child(
                    h_flex()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(t(L10nKey::StatisticsCommon)),
                        )
                        .child(groups_menu),
                )
                .children(groups.iter().skip(page * 10).take(10).map(|group| {
                    self.render_statistics_breakdown(group, false, analysis, maximum, cx)
                }))
                .when(groups.is_empty(), |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(tk.k5)
                            .child(t(L10nKey::StatisticsEmpty)),
                    )
                })
                .when(pages > 1, |common| {
                    common.child(
                        h_flex()
                            .gap_2()
                            .child(
                                self.analysis_button(
                                    "statistics-group-previous",
                                    "‹",
                                    cx,
                                    move |this, _, cx| {
                                        this.change_analysis(
                                            |s| s.page = page.saturating_sub(1),
                                            false,
                                            cx,
                                        )
                                    },
                                )
                                .accessibility_label(t(L10nKey::StatisticsPreviousPage))
                                .disabled(page == 0),
                            )
                            .child(div().text_xs().child(format!("{} / {pages}", page + 1)))
                            .child(
                                self.analysis_button(
                                    "statistics-group-next",
                                    "›",
                                    cx,
                                    move |this, _, cx| {
                                        this.change_analysis(|s| s.page = page + 1, false, cx)
                                    },
                                )
                                .accessibility_label(t(L10nKey::StatisticsNextPage))
                                .disabled(page + 1 >= pages),
                            ),
                    )
                });
        common.into_any_element()
    }

    fn render_statistics_agents(&self, analysis: &Analysis, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.active_settings().unwrap().statistics.analysis;
        let tk = Tk::of(cx);
        let agent_pages = analysis.agents().len().div_ceil(10).max(1);
        let agent_page = state.agent_page.min(agent_pages - 1);
        let agents = v_flex()
            .flex_grow(1.)
            .flex_shrink(1.)
            .flex_basis(rems(20.))
            .min_w_0()
            .gap_2()
            .py_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(t(L10nKey::StatisticsAgents)),
            )
            .child(
                h_flex()
                    .h_1p5()
                    .w_full()
                    .rounded_sm()
                    .overflow_hidden()
                    .bg(tk.k08)
                    .children(analysis.agents().iter().filter_map(|group| {
                        let total =
                            breakdown_value(analysis.totals(), state.metric).filter(|v| *v > 0.)?;
                        let value = breakdown_value(group.totals(), state.metric)?;
                        Some(
                            div()
                                .h_full()
                                .w(gpui_kit::relative((value / total).clamp(0., 1.) as f32))
                                .bg(cx.theme().primary.opacity(agent_opacity(group.agent()))),
                        )
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(tk.k5)
                    .child(div().flex_1().min_w_0().child(t(L10nKey::StatisticsAgent)))
                    .child(
                        div()
                            .w_12()
                            .text_right()
                            .child(t(L10nKey::StatisticsTokens)),
                    )
                    .child(
                        div()
                            .w_16()
                            .text_right()
                            .child(t(L10nKey::StatisticsCostCard)),
                    )
                    .child(
                        div()
                            .w_10()
                            .text_right()
                            .child(t(L10nKey::StatisticsSessionColumn)),
                    )
                    .child(div().w_8().text_right().child(t(L10nKey::StatisticsShare))),
            )
            .children(
                analysis
                    .agents()
                    .iter()
                    .skip(agent_page * 10)
                    .take(10)
                    .map(|group| self.render_statistics_breakdown(group, true, analysis, 0., cx)),
            )
            .when(analysis.agents().is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(tk.k5)
                        .child(t(L10nKey::StatisticsEmpty)),
                )
            })
            .when(agent_pages > 1, |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .child(
                            self.analysis_button(
                                "statistics-agents-previous",
                                "‹",
                                cx,
                                move |this, _, cx| {
                                    this.change_analysis(
                                        |s| s.agent_page = agent_page.saturating_sub(1),
                                        false,
                                        cx,
                                    )
                                },
                            )
                            .accessibility_label(t(L10nKey::StatisticsPreviousPage))
                            .disabled(agent_page == 0),
                        )
                        .child(
                            div()
                                .text_xs()
                                .child(format!("{} / {agent_pages}", agent_page + 1)),
                        )
                        .child(
                            self.analysis_button(
                                "statistics-agents-next",
                                "›",
                                cx,
                                move |this, _, cx| {
                                    this.change_analysis(
                                        |s| s.agent_page = agent_page + 1,
                                        false,
                                        cx,
                                    )
                                },
                            )
                            .accessibility_label(t(L10nKey::StatisticsNextPage))
                            .disabled(agent_page + 1 >= agent_pages),
                        ),
                )
            });
        agents.into_any_element()
    }

    fn render_statistics_breakdown(
        &self,
        group: &Breakdown,
        table: bool,
        analysis: &lyrel_core::statistics::analysis::Analysis,
        maximum: f64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let raw_label = group.label().unwrap_or(t(L10nKey::StatisticsUnattributed));
        let label = if table {
            agent_label(group.agent())
        } else {
            raw_label
        };
        let context = format!(
            "{} · {} · {}",
            group.agent(),
            group.scope(),
            group.provider().unwrap_or("")
        );
        let totals = group.totals();
        let state = &self.active_settings().unwrap().statistics.analysis;
        let key = format!(
            "{table}:{}:{}:{}:{raw_label}",
            group.agent(),
            group.scope(),
            group.provider().unwrap_or("")
        );
        let expanded = state.expanded.as_deref() == Some(&key);
        let metric_value = breakdown_value(totals, state.metric);
        let share = metric_value
            .zip(breakdown_value(analysis.totals(), state.metric))
            .filter(|(_, total)| *total > 0.)
            .map(|(value, total)| format!("{:.0}%", value / total * 100.))
            .unwrap_or_else(|| "—".into());
        let cost = breakdown_value(totals, Metric::Cost)
            .map(|v| {
                let estimated = totals
                    .estimated_costs()
                    .get("USD")
                    .and_then(|estimate| estimate.total())
                    .is_some();
                format!("{}${v:.2}", if estimated { "≈ " } else { "" })
            })
            .unwrap_or_else(|| "—".into());
        let primary_value = match state.metric {
            Metric::Tokens => dashboard::compact(totals.tokens().total()),
            Metric::Cost => cost.clone(),
            Metric::Sessions => session_count(totals),
        };
        let secondary = match state.metric {
            Metric::Tokens => t_fmt(
                L10nKey::StatisticsRankContext,
                &[("cost", &cost), ("sessions", &session_count(totals))],
            ),
            Metric::Cost => t_fmt(
                L10nKey::StatisticsRankUsage,
                &[
                    ("tokens", &dashboard::compact(totals.tokens().total())),
                    ("sessions", &session_count(totals)),
                ],
            ),
            Metric::Sessions => t_fmt(
                L10nKey::StatisticsRankSpend,
                &[
                    ("tokens", &dashboard::compact(totals.tokens().total())),
                    ("cost", &cost),
                ],
            ),
        };
        let identity = h_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .when(table, |row| {
                row.child(
                    div()
                        .size_1p5()
                        .rounded_sm()
                        .bg(cx.theme().primary.opacity(agent_opacity(group.agent()))),
                )
            })
            .child(div().truncate().child(label.to_owned()));
        let row = if table {
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .text_xs()
                .child(identity)
                .child(
                    div()
                        .w_12()
                        .text_right()
                        .child(dashboard::compact(totals.tokens().total())),
                )
                .child(
                    div()
                        .w_16()
                        .text_right()
                        .text_color(tk.k5)
                        .child(cost.clone()),
                )
                .child(
                    div()
                        .w_10()
                        .text_right()
                        .text_color(tk.k5)
                        .child(session_count(totals)),
                )
                .child(div().w_8().text_right().text_color(tk.k5).child(share))
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .text_xs()
                        .flex_wrap()
                        .child(identity)
                        .child(div().text_color(tk.k5).child(secondary))
                        .child(div().child(primary_value.clone())),
                )
                .child(
                    h_flex()
                        .w_full()
                        .h_0p5()
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(tk.k08)
                        .when_some(metric_value.filter(|_| maximum > 0.), |bar, value| {
                            bar.child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative((value / maximum).clamp(0., 1.) as f32))
                                    .bg(tk.k5),
                            )
                        }),
                )
                .into_any_element()
        };
        let description = detail(totals);
        let content = v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .whitespace_normal()
            .text_left()
            .child(row)
            .when(expanded, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!(
                            "statistics-breakdown-detail:{key}"
                        )))
                        .text_xs()
                        .text_color(tk.k5)
                        .child(format!(
                            "{context}\n{description}\n{}",
                            t(L10nKey::StatisticsBreakdownScope)
                        )),
                )
            });
        Button::new(SharedString::from(format!("statistics-breakdown:{key}")))
            .ghost()
            .h_auto()
            .w_full()
            .min_w_0()
            .py_1p5()
            .px_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .accessibility_label(format!(
                "{label} · {primary_value}\n{context}\n{description}"
            ))
            .tooltip(context)
            .child(content)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.change_analysis(
                    |s| s.expanded = if expanded { None } else { Some(key.clone()) },
                    false,
                    cx,
                );
            }))
            .into_any_element()
    }
}
