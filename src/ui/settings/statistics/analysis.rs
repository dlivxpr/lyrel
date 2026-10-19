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
use lyrel_core::statistics::analysis::{Breakdown, Metric};

pub(super) struct AnalysisState {
    pub(super) agent: Option<String>,
    pub(super) year: i32,
    pub(super) date: NaiveDate,
    pub(super) detail: bool,
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
        Self {
            agent: None,
            year: date.year(),
            date,
            detail: false,
            summary_detail: false,
            focus: cx.focus_handle().tab_stop(true),
            anchor: None,
            calendar_scroll: gpui_kit::ScrollHandle::new(),
            metric: Metric::Tokens,
            group: GroupKind::Models,
            page: 0,
            expanded: None,
            agent_page: 0,
        }
    }
    fn reveal_date(&self) {
        let offset = NaiveDate::from_ymd_opt(self.year, 1, 1)
            .unwrap()
            .weekday()
            .num_days_from_monday();
        self.calendar_scroll
            .scroll_to_item(((offset + self.date.ordinal0()) / 7 + 1) as usize);
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
        state.reveal_date();
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
            MenuEntry::item(
                agent.clone(),
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
                .disabled(year >= 9998),
            );
        let mut activity = v_flex()
            .id("statistics-activity")
            .min_w_0()
            .w_full()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_xl()
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t(L10nKey::StatisticsActivity)),
                    )
                    .child(year_controls)
                    .child(div().flex_1())
                    .child(metric),
            )
            .child(h_flex().min_w_0().flex_wrap().gap_2().child(agent).when(
                p.analysis.as_ref().and_then(|a| a.agent_filter()) != state.agent.as_deref(),
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
            ));
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
                    .text_color(tk.warn_text)
                    .child(t(L10nKey::StatisticsNoAnalysisSources)),
            );
        }
        let maximum = analysis.maximum(state.metric);
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
                // Keep days square; narrow windows scroll the year instead of crushing it.
                v_flex()
                    .w_3()
                    .flex_shrink_0()
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
                        let value = day.value(state.metric);
                        let background = match value {
                            Some(v) if v > 0. && maximum > 0. => {
                                cx.theme().primary.opacity(0.2 + 0.8 * (v / maximum) as f32)
                            }
                            _ => tk.k06,
                        };
                        let description = if !day.is_in_range() {
                            t(L10nKey::StatisticsOutsideRange).to_owned()
                        } else if value.is_none() {
                            t(L10nKey::StatisticsUnknown).to_owned()
                        } else {
                            match state.metric {
                                Metric::Tokens => {
                                    number(day.totals().tokens().total().or_else(|| {
                                        (day.totals().calls() == 0 && day.totals().is_complete())
                                            .then_some(0)
                                    }))
                                }
                                Metric::Cost => format!("USD {:.4}", value.unwrap()),
                                Metric::Sessions => session_count(day.totals()),
                            }
                        };
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
                            } else {
                                background
                            })
                            .when(!day.is_in_range(), |b| b.opacity(0.35))
                            .when(day.is_in_range() && value.is_none(), |b| b.label("?"))
                            .accessibility_label(name.clone())
                            .tooltip(name)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_analysis_date(date, window, cx)
                            }))
                            .into_any_element()
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let grid = v_flex()
            .id("statistics-calendar")
            .relative()
            .track_focus(&state.focus)
            .role(gpui_kit::Role::Group)
            .aria_label(t(L10nKey::StatisticsCalendarKeys))
            .min_w_0()
            .gap_2()
            .flex_shrink_0()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .p_1()
            .pb_3()
            .focus_visible(move |s| s.border_color(accent))
            .on_key_down(cx.listener(Self::analysis_calendar_key))
            .child(
                h_flex()
                    .id("statistics-year-weeks")
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .overflow_x_scroll()
                    .track_scroll(&state.calendar_scroll)
                    .gap_0p5()
                    .child(
                        v_flex()
                            .w_4()
                            .flex_shrink_0()
                            .gap_0p5()
                            .child(div().h_4())
                            .children(t(L10nKey::StatisticsWeekdays).split_whitespace().map(
                                |label| {
                                    div()
                                        .h_3()
                                        .text_xs()
                                        .text_color(tk.k5)
                                        .child(label.to_owned())
                                },
                            )),
                    )
                    .children(columns),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::horizontal(&state.calendar_scroll)),
            );
        let date = state.date;
        let selected = analysis.days().iter().find(|d| d.date() == date);
        activity =
            activity
                .child(grid)
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            div()
                                .id("statistics-date-anchor")
                                .anchor_scroll(state.anchor.clone())
                                .child(
                                    self.analysis_button(
                                        "statistics-calendar-focus",
                                        date.to_string(),
                                        cx,
                                        move |this, window, cx| {
                                            this.select_analysis_date(date, window, cx)
                                        },
                                    )
                                    .tooltip(format!(
                                        "{}\n{}",
                                        t(L10nKey::StatisticsCalendarKeys),
                                        t(L10nKey::StatisticsMetricScope)
                                    )),
                                ),
                        )
                        .child(div().text_xs().text_color(tk.k5).child(t_fmt(
                            L10nKey::StatisticsActivityStreaks,
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
                        )))
                        .child(div().flex_1())
                        .child(h_flex().gap_0p5().children([0., 0.25, 0.5, 0.75, 1.].map(
                            |level| {
                                div().size_2().rounded_sm().bg(if level == 0. {
                                    tk.k06
                                } else {
                                    accent.opacity(level)
                                })
                            },
                        ))),
                )
                .child(
                    div().text_xs().text_color(tk.k5).child(t_fmt(
                        L10nKey::StatisticsHeatmapLegend,
                        &[
                            ("metric", t(metric_key(state.metric))),
                            (
                                "max",
                                &if analysis
                                    .days()
                                    .iter()
                                    .all(|day| day.value(state.metric).is_none())
                                {
                                    t(L10nKey::StatisticsUnknown).into()
                                } else if state.metric == Metric::Cost {
                                    format!("USD {maximum:.4}")
                                } else {
                                    format!("{maximum:.0}")
                                },
                            ),
                        ],
                    )),
                );
        if state.detail {
            let text = selected
                .map(|d| {
                    if d.is_in_range() {
                        detail(d.totals())
                    } else {
                        t(L10nKey::StatisticsOutsideRange).into()
                    }
                })
                .unwrap_or_else(|| t(L10nKey::StatisticsLoading).into());
            activity = activity.child(div().text_xs().child(text));
        }
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
        let groups = match state.group {
            GroupKind::Models => analysis.models(),
            GroupKind::Projects => analysis.projects(),
        };
        let maximum = groups
            .iter()
            .filter_map(|group| state.metric.value(group.totals()))
            .fold(0., f64::max);
        let pages = groups.len().div_ceil(10).max(1);
        let page = state.page.min(pages - 1);
        let common =
            v_flex()
                .flex_grow(1.)
                .flex_shrink(1.)
                .flex_basis(rems(21.))
                .min_w_0()
                .gap_2()
                .p_3()
                .border_1()
                .border_color(cx.theme().border)
                .rounded_xl()
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
        let agent_pages = analysis.agents().len().div_ceil(10).max(1);
        let agent_page = state.agent_page.min(agent_pages - 1);
        let agents = v_flex()
            .flex_grow(1.)
            .flex_shrink(1.)
            .flex_basis(rems(21.))
            .min_w_0()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_xl()
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
                        let total = state.metric.value(analysis.totals()).filter(|v| *v > 0.)?;
                        let value = state.metric.value(group.totals())?;
                        Some(
                            div()
                                .h_full()
                                .w(gpui_kit::relative((value / total) as f32))
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
        v_flex()
            .min_w_0()
            .gap_4()
            .child(activity)
            .child(
                h_flex()
                    .items_start()
                    .flex_wrap()
                    .gap_3()
                    .child(agents)
                    .child(common),
            )
            .into_any_element()
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
        let metric_value = state.metric.value(totals);
        let share = metric_value
            .zip(state.metric.value(analysis.totals()))
            .filter(|(_, total)| *total > 0.)
            .map(|(value, total)| format!("{:.0}%", value / total * 100.))
            .unwrap_or_else(|| "—".into());
        let cost = totals
            .recorded_usd()
            .map(|v| format!("${v:.2}"))
            .unwrap_or_else(|| "—".into());
        let primary_value = match state.metric {
            Metric::Tokens => dashboard::compact(totals.tokens().total()),
            Metric::Cost => cost.clone(),
            Metric::Sessions => session_count(totals),
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
                        .gap_2()
                        .text_xs()
                        .child(identity)
                        .child(div().text_color(tk.k5).child(t_fmt(
                            L10nKey::StatisticsRankContext,
                            &[("cost", &cost), ("sessions", &session_count(totals))],
                        )))
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
                                    .w(gpui_kit::relative((value / maximum) as f32))
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
