//! 统计页内分析交互；不拥有第二份来源、缓存或聚合算法。
use super::*;
use chrono::{Datelike, Days, Months, NaiveDate};
use gpui_kit::FocusHandle;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    button::Button,
    menu::{DropdownMenu as _, PopupMenuItem},
};
use lyrel_core::statistics::analysis::{Breakdown, Metric};

pub(super) struct AnalysisState {
    pub(super) agent: Option<String>,
    pub(super) year: i32,
    pub(super) date: NaiveDate,
    pub(super) detail: bool,
    pub(super) focus: FocusHandle,
    pub(super) anchor: Option<gpui_kit::ScrollAnchor>,
    pub(super) metric: Metric,
    group: GroupKind,
    page: usize,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Agents,
    Models,
    Projects,
}
impl AnalysisState {
    pub(super) fn new(cx: &mut Context<LyrelApp>) -> Self {
        let date = chrono::Local::now().date_naive();
        Self {
            agent: None,
            year: date.year(),
            date,
            detail: true,
            focus: cx.focus_handle().tab_stop(true),
            anchor: None,
            metric: Metric::Tokens,
            group: GroupKind::Agents,
            page: 0,
        }
    }
}
fn metric_key(metric: Metric) -> L10nKey {
    match metric {
        Metric::Tokens => L10nKey::StatisticsTokens,
        Metric::Cost => L10nKey::StatisticsCost,
        Metric::Sessions => L10nKey::StatisticsSessions,
    }
}
fn group_key(group: GroupKind) -> L10nKey {
    match group {
        GroupKind::Agents => L10nKey::StatisticsAgents,
        GroupKind::Models => L10nKey::StatisticsModels,
        GroupKind::Projects => L10nKey::StatisticsProjects,
    }
}
fn detail(totals: &Totals) -> String {
    let empty = totals.calls() == 0 && totals.is_complete();
    let tokens = |value: Option<u64>| number(value.or_else(|| empty.then_some(0)));
    t_fmt(
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
            ("sessions", &totals.main_sessions().to_string()),
            ("calls", &totals.calls().to_string()),
            ("cumulative", &totals.cumulative_records().to_string()),
            ("unpriced", &totals.unpriced_calls().to_string()),
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
    )
}
impl LyrelApp {
    fn analysis_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Button {
        Button::new(id)
            .label(label)
            .small()
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
    }
    fn analysis_menu(
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
    fn change_analysis(
        &mut self,
        change: impl FnOnce(&mut AnalysisState),
        reload: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.active_settings_mut() {
            change(&mut s.statistics.analysis);
            if reload {
                s.statistics.invalidate();
            }
        }
        if reload {
            self.load_statistics(cx);
        }
        cx.notify();
    }
    fn select_analysis_date(
        &mut self,
        date: NaiveDate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.active_settings_mut() {
            s.statistics.analysis.date = date;
            s.statistics.analysis.detail = true;
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
            "left" => date.checked_sub_days(Days::new(1)),
            "right" => date.checked_add_days(Days::new(1)),
            "up" => date.checked_sub_days(Days::new(7)),
            "down" => date.checked_add_days(Days::new(7)),
            "pageup" => date.checked_sub_months(Months::new(1)),
            "pagedown" => date.checked_add_months(Months::new(1)),
            "home" => NaiveDate::from_ymd_opt(state.year, 1, 1),
            "end" => NaiveDate::from_ymd_opt(state.year, 12, 31),
            "enter" | "space" => {
                state.detail = true;
                Some(date)
            }
            "escape" if state.detail => {
                state.detail = false;
                Some(date)
            }
            _ => return,
        };
        if let Some(date) = moved.filter(|d| d.year() == state.year) {
            state.date = date;
        }
        if let Some(anchor) = &state.anchor {
            anchor.scroll_to(window, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(super) fn render_statistics_analysis(
        &self,
        p: &Presentation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.active_settings().unwrap().statistics.analysis;
        let tk = Tk::of(cx);
        let border = cx.theme().border;
        let accent = cx.theme().primary;
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
            let selected = state.agent.as_deref() == Some(&agent);
            MenuEntry::item(agent.clone(), selected, move |this, _, cx| {
                this.change_analysis(
                    |s| {
                        s.agent = Some(agent.clone());
                        s.page = 0;
                    },
                    true,
                    cx,
                )
            })
        }));
        let agent = self.analysis_menu(
            "statistics-analysis-agent",
            state
                .agent
                .clone()
                .unwrap_or_else(|| t(L10nKey::StatisticsAllAgents).into()),
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
        let mut rows = vec![
            div()
                .text_size(fs(12.))
                .text_color(tk.k5)
                .child(t(L10nKey::StatisticsAnalysisTitle))
                .into_any_element(),
            self.settings_row(
                t(L10nKey::StatisticsAnalysisAgent),
                t(L10nKey::StatisticsAnalysisScope),
                agent,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(L10nKey::StatisticsAnalysisMetric),
                t(L10nKey::StatisticsMetricScope),
                metric,
                cx,
            )
            .into_any_element(),
        ];
        if let Some(error) = &p.analysis_error {
            rows.push(
                div()
                    .text_color(tk.danger)
                    .child(t_fmt(L10nKey::StatisticsAnalysisError, &[("error", error)]))
                    .into_any_element(),
            );
        }
        let Some(analysis) = &p.analysis else {
            return v_flex().gap_3().children(rows).into_any_element();
        };
        rows.push(
            div()
                .text_color(tk.k5)
                .child(t_fmt(
                    L10nKey::StatisticsAnalysisSnapshot,
                    &[(
                        "agent",
                        analysis
                            .agent_filter()
                            .unwrap_or(t(L10nKey::StatisticsAllAgents)),
                    )],
                ))
                .into_any_element(),
        );
        if !analysis.has_sources() {
            rows.push(
                div()
                    .text_color(tk.warn_text)
                    .child(t(L10nKey::StatisticsNoAnalysisSources))
                    .into_any_element(),
            );
        }
        rows.push(div().child(detail(analysis.totals())).into_any_element());
        let year = state.year;
        rows.push(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    self.analysis_button(
                        "statistics-year-previous",
                        t(L10nKey::StatisticsPreviousYear),
                        cx,
                        move |this, _, cx| {
                            this.change_analysis(
                                |s| {
                                    s.year = year - 1;
                                    s.date = NaiveDate::from_ymd_opt(s.year, 1, 1).unwrap();
                                },
                                true,
                                cx,
                            )
                        },
                    )
                    .disabled(year <= 1),
                )
                .child(analysis.year().to_string())
                .child(
                    self.analysis_button(
                        "statistics-year-next",
                        t(L10nKey::StatisticsNextYear),
                        cx,
                        move |this, _, cx| {
                            this.change_analysis(
                                |s| {
                                    s.year = year + 1;
                                    s.date = NaiveDate::from_ymd_opt(s.year, 1, 1).unwrap();
                                },
                                true,
                                cx,
                            )
                        },
                    )
                    .disabled(year >= 9998),
                )
                .into_any_element(),
        );
        rows.push(
            div()
                .text_color(tk.k5)
                .child(t_fmt(
                    L10nKey::StatisticsStreaks,
                    &[
                        (
                            "current",
                            &number(analysis.streaks().current().map(u64::from)),
                        ),
                        (
                            "longest",
                            &number(analysis.streaks().longest().map(u64::from)),
                        ),
                    ],
                ))
                .into_any_element(),
        );
        let selected = analysis.days().iter().find(|d| d.date() == state.date);
        let date = state.date;
        let selected_text = selected
            .map(|d| {
                if !d.is_in_range() {
                    t(L10nKey::StatisticsOutsideRange).to_owned()
                } else {
                    detail(d.totals())
                }
            })
            .unwrap_or_else(|| t(L10nKey::StatisticsLoading).into());
        let maximum = analysis.maximum(state.metric);
        let scale = if state.metric == Metric::Cost {
            format!("USD {maximum:.4}")
        } else {
            format!("{maximum:.0}")
        };
        // The focused date inspector is before the year grid, so keyboard navigation always
        // exposes the full date and values without relying on hover or a clipped grid cell.
        rows.push(
            v_flex()
                .id("statistics-calendar")
                .track_focus(&state.focus)
                .min_w_0()
                .border_1()
                .border_color(border)
                .rounded_md()
                .p_2()
                .gap_2()
                .focus_visible(|s| s.border_color(accent))
                .on_key_down(cx.listener(Self::analysis_calendar_key))
                .child(
                    div()
                        .id("statistics-date-anchor")
                        .anchor_scroll(state.anchor.clone())
                        .child(self.analysis_button(
                            "statistics-calendar-focus",
                            date.format("%Y-%m-%d").to_string(),
                            cx,
                            move |this, window, cx| this.select_analysis_date(date, window, cx),
                        )),
                )
                .child(
                    div()
                        .text_color(tk.k5)
                        .child(t(L10nKey::StatisticsCalendarKeys)),
                )
                .when(state.detail, |el| el.child(div().child(selected_text)))
                .child(div().text_color(tk.k5).child(t_fmt(
                    L10nKey::StatisticsHeatmapLegend,
                    &[("metric", t(metric_key(state.metric))), ("max", &scale)],
                )))
                .child(
                    h_flex()
                        .id("statistics-year-months")
                        .min_w_0()
                        .w_full()
                        .overflow_x_scroll()
                        .items_start()
                        .flex_wrap()
                        .gap_4()
                        .children((1..=12).map(|month| {
                            let month_days: Vec<_> = analysis
                                .days()
                                .iter()
                                .filter(|d| d.date().month() == month)
                                .collect();
                            let offset =
                                month_days[0].date().weekday().num_days_from_monday() as usize;
                            let mut cells: Vec<AnyElement> = (0..offset)
                                .map(|_| div().w_8().h_8().into_any_element())
                                .collect();
                            cells.extend(month_days.into_iter().map(|day| {
                                let date = day.date();
                                let value = day.value(state.metric);
                                let background = match value {
                                    Some(v) if v > 0. && maximum > 0. => {
                                        accent.opacity(0.12 + 0.24 * (v / maximum) as f32)
                                    }
                                    _ => tk.k06,
                                };
                                let suffix = if !day.is_in_range() {
                                    "–"
                                } else if value.is_none() {
                                    "?"
                                } else {
                                    ""
                                };
                                Button::new(SharedString::from(format!("statistics-day-{date}")))
                                    .label(format!("{}{suffix}", date.day()))
                                    .small()
                                    .w_8()
                                    .h_8()
                                    .px_0()
                                    .bg(background)
                                    .border_1()
                                    .border_color(if date == state.date { accent } else { border })
                                    .when(!day.is_in_range(), |b| b.text_color(tk.k5))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_analysis_date(date, window, cx)
                                    }))
                                    .into_any_element()
                            }));
                            v_flex()
                                .w_64()
                                .flex_shrink_0()
                                .gap_2()
                                .child(format!("{}-{month:02}", analysis.year()))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .text_color(tk.k5)
                                        .text_size(fs(12.))
                                        .children(
                                            t(L10nKey::StatisticsWeekdays).split_whitespace().map(
                                                |label| div().w_8().text_center().child(label),
                                            ),
                                        ),
                                )
                                .child(h_flex().items_start().flex_wrap().gap_1().children(cells))
                                .into_any_element()
                        })),
                )
                .into_any_element(),
        );
        let groups = self.analysis_menu(
            "statistics-analysis-group",
            t(group_key(state.group)),
            [GroupKind::Agents, GroupKind::Models, GroupKind::Projects]
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
        rows.push(
            self.settings_row(
                t(L10nKey::StatisticsBreakdowns),
                t(L10nKey::StatisticsBreakdownScope),
                groups,
                cx,
            )
            .into_any_element(),
        );
        let groups: &[Breakdown] = match state.group {
            GroupKind::Agents => analysis.agents(),
            GroupKind::Models => analysis.models(),
            GroupKind::Projects => analysis.projects(),
        };
        let pages = groups.len().div_ceil(10).max(1);
        let page = state.page.min(pages - 1);
        for group in groups.iter().skip(page * 10).take(10) {
            rows.push(
                v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div().font_weight(FontWeight::MEDIUM).child(
                            group
                                .label()
                                .unwrap_or(t(L10nKey::StatisticsUnattributed))
                                .to_owned(),
                        ),
                    )
                    .child(div().text_color(tk.k5).child(format!(
                        "{} · {}\n{}",
                        group.agent(),
                        group.scope(),
                        group.provider().unwrap_or("")
                    )))
                    .child(detail(group.totals()))
                    .into_any_element(),
            );
        }
        if groups.is_empty() {
            rows.push(
                div()
                    .text_color(tk.k5)
                    .child(t(L10nKey::StatisticsEmpty))
                    .into_any_element(),
            );
        }
        rows.push(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    self.analysis_button(
                        "statistics-group-previous",
                        t(L10nKey::StatisticsPreviousPage),
                        cx,
                        move |this, _, cx| {
                            this.change_analysis(|s| s.page = page.saturating_sub(1), false, cx)
                        },
                    )
                    .disabled(page == 0),
                )
                .child(format!("{} / {pages}", page + 1))
                .child(
                    self.analysis_button(
                        "statistics-group-next",
                        t(L10nKey::StatisticsNextPage),
                        cx,
                        move |this, _, cx| this.change_analysis(|s| s.page = page + 1, false, cx),
                    )
                    .disabled(page + 1 >= pages),
                )
                .into_any_element(),
        );
        v_flex().gap_3().min_w_0().children(rows).into_any_element()
    }
}
