//! 总览沿用唯一 Totals／Analysis；未知与部分数据不补零、不绘制虚构趋势。
use super::*;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, button::Button};
use gpui_kit::{PathBuilder, canvas, point, relative, rems};
use lyrel_core::statistics::analysis::Metric;

pub(super) fn compact(value: Option<u64>) -> String {
    match value {
        Some(v) if v >= 1_000_000_000 => format!("{:.2}B", v as f64 / 1_000_000_000.),
        Some(v) if v >= 1_000_000 => format!("{:.1}M", v as f64 / 1_000_000.),
        Some(v) if v >= 10_000 => format!("{:.1}K", v as f64 / 1_000.),
        other => number(other),
    }
}

fn recorded_value(totals: &Totals) -> Option<f64> {
    // A recorded-cost chart must not silently include custom/catalog/native estimates.
    (totals.is_complete() && totals.estimated_costs().is_empty() && totals.unpriced_calls() == 0)
        .then(|| {
            totals
                .recorded_usd()
                .or_else(|| (totals.calls() == 0).then_some(0.))
        })
        .flatten()
}

fn comparison(totals: &Totals, previous: Option<&Totals>, metric: Metric) -> Option<f64> {
    let previous = previous?;
    if metric == Metric::Tokens {
        return totals.token_change(previous);
    }
    let (current, previous) = if metric == Metric::Cost {
        (recorded_value(totals)?, recorded_value(previous)?)
    } else {
        (
            Metric::Sessions.value(totals)?,
            Metric::Sessions.value(previous)?,
        )
    };
    (previous > 0.).then(|| (current / previous - 1.) * 100.)
}

impl LyrelApp {
    pub(super) fn statistics_range_label(&self, p: &Presentation) -> String {
        match (p.window.start(), p.window.end()) {
            (Some(start), Some(end)) => format!(
                "{} — {}",
                chrono::DateTime::from_timestamp(start, 0)
                    .unwrap()
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d"),
                chrono::DateTime::from_timestamp(end - 1, 0)
                    .unwrap()
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d"),
            ),
            _ => t(L10nKey::StatisticsAll).into(),
        }
    }

    fn render_statistics_composition(
        &self,
        totals: Option<&Totals>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let tokens = totals.map(Totals::tokens);
        let parts = [
            (L10nKey::StatisticsInput, tokens.and_then(|v| v.input())),
            (L10nKey::StatisticsOutput, tokens.and_then(|v| v.output())),
            (
                L10nKey::StatisticsCacheRead,
                tokens.and_then(|v| v.cache_read()),
            ),
            (
                L10nKey::StatisticsCacheWrite,
                tokens.and_then(|v| v.cache_write()),
            ),
        ];
        // Reasoning may overlap output. Only the four proven additive categories form the bar.
        let sum = parts
            .iter()
            .try_fold(0_u64, |sum, (_, value)| sum.checked_add((*value)?));
        let total = totals
            .filter(|v| v.is_complete())
            .and_then(|v| v.tokens().total())
            .filter(|v| *v > 0 && Some(*v) == sum);
        let colors = [
            cx.theme().primary,
            cx.theme().primary.opacity(0.7),
            cx.theme().primary.opacity(0.3),
            cx.theme().foreground.opacity(0.65),
        ];
        let bar = h_flex()
            .id("statistics-token-composition")
            .role(gpui_kit::Role::Group)
            .aria_label(
                total
                    .map(|value| {
                        format!("{}: {}", t(L10nKey::StatisticsTokens), number(Some(value)))
                    })
                    .unwrap_or_else(|| t(L10nKey::StatisticsCompositionUnavailable).into()),
            )
            .h_1p5()
            .w_full()
            .rounded_sm()
            .overflow_hidden()
            .bg(tk.k08)
            .when_some(total, |bar, total| {
                bar.children(parts.iter().zip(colors).filter_map(|((_, value), color)| {
                    let value = (*value)?;
                    (value > 0).then(|| {
                        div()
                            .h_full()
                            .w(relative(value as f32 / total as f32))
                            .bg(color)
                    })
                }))
            })
            .test_support();
        v_flex()
            .gap_1()
            .child(bar)
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .gap_1()
                    .text_size(rems(11. / 16.))
                    .text_color(tk.k5)
                    .children(
                        parts
                            .into_iter()
                            .zip(colors)
                            .map(|((label, value), color)| {
                                let value = total.zip(value).map_or_else(
                                    || compact(value),
                                    |(total, value)| {
                                        format!("{:.0}%", value as f64 / total as f64 * 100.)
                                    },
                                );
                                h_flex()
                                    .gap_0p5()
                                    .child(div().size_1().rounded_sm().bg(color))
                                    .child(format!("{} {value}", t(label)))
                            }),
                    ),
            )
            .when(total.is_none(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(tk.k5)
                        .child(t(L10nKey::StatisticsCompositionUnavailable)),
                )
            })
            .into_any_element()
    }

    fn render_statistics_mini_trend(
        &self,
        presentation: Option<&Presentation>,
        metric: Metric,
        cx: &Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        // Analysis owns local-calendar attribution. Never distribute cumulative totals over dates.
        let today = chrono::Local::now().date_naive();
        let days: Vec<_> = presentation
            .and_then(|p| p.analysis.as_ref())
            .filter(|a| a.agent_filter().is_none())
            .into_iter()
            .flat_map(|a| a.days())
            .filter(|day| day.is_in_range())
            .filter(|day| day.date() <= today)
            .map(|day| {
                (
                    day.date(),
                    if metric == Metric::Cost {
                        recorded_value(day.totals())
                    } else {
                        day.value(metric)
                    },
                )
            })
            .collect();
        let has_segment = days
            .windows(2)
            .any(|pair| pair.iter().all(|(_, value)| value.is_some()));
        let caption = if has_segment {
            t_fmt(
                L10nKey::StatisticsDailyTrend,
                &[
                    ("start", &days.first().unwrap().0.to_string()),
                    ("end", &days.last().unwrap().0.to_string()),
                ],
            )
        } else {
            t(L10nKey::StatisticsTrendUnavailable).to_owned()
        };
        let color = if metric == Metric::Cost {
            cx.theme().primary
        } else {
            tk.k5
        };
        let id = if metric == Metric::Cost {
            "statistics-cost-trend"
        } else {
            "statistics-session-trend"
        };
        let mut region = v_flex()
            .id(id)
            .w_full()
            .min_w_0()
            .gap_1()
            .role(gpui_kit::Role::Group)
            .aria_label(caption.clone());
        if has_segment {
            region = region.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let maximum = days
                            .iter()
                            .filter_map(|(_, value)| *value)
                            .fold(0., f64::max);
                        let mut path = PathBuilder::stroke(window.rem_size() / 16.);
                        let mut connected = false;
                        for (ix, (_, value)) in days.iter().enumerate() {
                            let Some(value) = value else {
                                connected = false;
                                continue;
                            };
                            let position = point(
                                bounds.left()
                                    + bounds.size.width * (ix as f32 / (days.len() - 1) as f32),
                                bounds.bottom()
                                    - window.rem_size() / 16.
                                    - (bounds.size.height - window.rem_size() / 8.)
                                        * if maximum > 0. {
                                            (*value / maximum) as f32
                                        } else {
                                            0.
                                        },
                            );
                            if connected {
                                path.line_to(position);
                            } else {
                                path.move_to(position);
                            }
                            connected = true;
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    },
                )
                .w_full()
                .h_7(),
            );
        } else {
            region = region.child(div().h_7());
        }
        region
            .child(
                div()
                    .text_size(rems(10. / 16.))
                    .text_color(tk.k5)
                    .child(caption),
            )
            .test_support()
            .into_any_element()
    }

    pub(super) fn render_statistics_tiles(
        &self,
        presentation: Option<&Presentation>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let state = &self.active_settings().unwrap().statistics.analysis;
        let metric = state.metric;
        let expanded = state.summary_detail;
        let totals = presentation.map(|p| &p.totals);
        let previous = presentation.and_then(|p| p.previous.as_ref());
        let tiles = h_flex()
            .id("statistics-tiles")
            .w_full()
            .min_w_0()
            .items_stretch()
            .flex_wrap()
            .gap_3()
            .children(
                [
                    (
                        "statistics-tile-tokens",
                        Metric::Tokens,
                        L10nKey::StatisticsTokens,
                    ),
                    (
                        "statistics-tile-cost",
                        Metric::Cost,
                        L10nKey::StatisticsCostCard,
                    ),
                    (
                        "statistics-tile-sessions",
                        Metric::Sessions,
                        L10nKey::StatisticsSessions,
                    ),
                ]
                .into_iter()
                .map(|(id, selected_metric, key)| {
                    let value = match selected_metric {
                        Metric::Tokens => compact(totals.and_then(|t| t.tokens().total())),
                        Metric::Cost => totals
                            .and_then(Totals::recorded_usd)
                            .map(|v| format!("USD {v:.2}"))
                            .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                        Metric::Sessions => totals
                            .map(session_count)
                            .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                    };
                    let change = totals
                        .and_then(|t| comparison(t, previous, selected_metric))
                        .map(|v| format!("{v:+.0}%"));
                    let comparison_hint = t_fmt(
                        L10nKey::StatisticsTokenDelta,
                        &[(
                            "change",
                            change
                                .as_deref()
                                .unwrap_or(t(L10nKey::StatisticsNotComparable)),
                        )],
                    );
                    let subtitle = match selected_metric {
                        Metric::Tokens => t_fmt(
                            L10nKey::StatisticsSessionAverage,
                            &[(
                                "value",
                                &totals
                                    .and_then(Totals::average_tokens)
                                    .map(|v| compact(Some(v.round() as u64)))
                                    .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                            )],
                        ),
                        Metric::Cost => t_fmt(
                            L10nKey::StatisticsRecordedSubtotal,
                            &[(
                                "count",
                                &totals
                                    .map(|v| v.unpriced_calls().to_string())
                                    .unwrap_or_else(|| "—".into()),
                            )],
                        ),
                        Metric::Sessions => t_fmt(
                            L10nKey::StatisticsRecordCount,
                            &[
                                (
                                    "calls",
                                    &totals
                                        .map(|v| v.calls().to_string())
                                        .unwrap_or_else(|| "—".into()),
                                ),
                                (
                                    "cumulative",
                                    &totals
                                        .map(|v| v.cumulative_records().to_string())
                                        .unwrap_or_else(|| "—".into()),
                                ),
                            ],
                        ),
                    };
                    let visualization = if selected_metric == Metric::Tokens {
                        self.render_statistics_composition(totals, cx)
                    } else {
                        self.render_statistics_mini_trend(presentation, selected_metric, cx)
                    };
                    let content = v_flex()
                        .w_full()
                        .min_w_0()
                        .min_h(rems(8.5))
                        .gap_1()
                        .whitespace_normal()
                        .text_left()
                        .child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(tk.k5)
                                .child(t(key))
                                .child(div().child(change.unwrap_or_else(|| "—".into()))),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("{id}-value")))
                                .text_2xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(value),
                        )
                        .child(div().text_xs().text_color(tk.k5).child(subtitle))
                        .child(div().mt_auto().pt_3().child(visualization));
                    Button::new(id)
                        .ghost()
                        .h_auto()
                        .flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(rems(13.))
                        .min_w_0()
                        .min_h(rems(10.))
                        .p_3()
                        .rounded_xl()
                        .bg(tk.k06)
                        .selected(metric == selected_metric)
                        .accessibility_label(t(key))
                        .tooltip(comparison_hint)
                        .child(content)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_analysis(|s| s.metric = selected_metric, false, cx);
                        }))
                        .into_any_element()
                }),
            );
        let mut summary = v_flex().min_w_0().gap_1().child(tiles);
        if let Some(totals) = totals {
            summary = summary.child(
                h_flex().justify_end().child(
                    self.analysis_button(
                        "statistics-summary-details",
                        t(L10nKey::StatisticsSummaryDetails),
                        cx,
                        move |this, _, cx| {
                            this.change_analysis(|s| s.summary_detail = !expanded, false, cx)
                        },
                    )
                    .selected(expanded),
                ),
            );
            if expanded {
                let description = analysis::detail(totals);
                summary = summary.child(
                    div()
                        .id("statistics-summary-values")
                        .text_xs()
                        .text_color(tk.k5)
                        .role(gpui_kit::Role::Group)
                        .aria_label(description.clone())
                        .whitespace_normal()
                        .child(description)
                        .test_support(),
                );
            }
        }
        summary.into_any_element()
    }
}
