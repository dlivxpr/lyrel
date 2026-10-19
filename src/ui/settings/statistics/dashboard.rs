//! 总览只展示同一聚合结果，不推断趋势、不混加费用来源或币种。
use super::*;
use gpui_kit::component::{Selectable as _, button::Button};
use lyrel_core::statistics::analysis::Metric;
use gpui_kit::rems;

impl LyrelApp {
    pub(super) fn statistics_range_label(&self, p: &Presentation) -> String {
        match (p.window.start(), p.window.end()) {
            (Some(start), Some(end)) => format!(
                "{} — {}",
                chrono::DateTime::from_timestamp(start, 0).unwrap()
                    .with_timezone(&chrono::Local).format("%Y-%m-%d"),
                chrono::DateTime::from_timestamp(end - 1, 0).unwrap()
                    .with_timezone(&chrono::Local).format("%Y-%m-%d"),
            ),
            _ => t(L10nKey::StatisticsAll).into(),
        }
    }

    pub(super) fn render_statistics_tiles(
        &self,
        presentation: Option<&Presentation>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let metric = self.active_settings().unwrap().statistics.analysis.metric;
        h_flex().id("statistics-tiles").w_full().min_w_0().items_stretch().gap_3()
            .children([
                ("statistics-tile-tokens", Metric::Tokens, L10nKey::StatisticsTokens),
                ("statistics-tile-cost", Metric::Cost, L10nKey::StatisticsCostCard),
                ("statistics-tile-sessions", Metric::Sessions, L10nKey::StatisticsSessions),
            ].into_iter().map(|(id, selected_metric, key)| {
                let totals = presentation.map(|p| &p.totals);
                let previous = presentation.and_then(|p| p.previous.as_ref());
                let value = match selected_metric {
                    Metric::Tokens => number(totals.and_then(|t| t.tokens().total())),
                    Metric::Cost => money(totals.and_then(Totals::recorded_usd)),
                    Metric::Sessions => totals.map(session_count)
                        .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()),
                };
                let mut content = v_flex().w_full().min_w_0().gap_1().whitespace_normal().text_left()
                    .child(div().text_xs().text_color(tk.k5).child(t(key)))
                    .child(div().id(SharedString::from(format!("{id}-value")))
                        .text_2xl().font_weight(FontWeight::SEMIBOLD).child(value));
                if let Some(totals) = totals {
                    match selected_metric {
                        Metric::Tokens => {
                            for (label, value) in [
                                (L10nKey::StatisticsInput, totals.tokens().input()),
                                (L10nKey::StatisticsOutput, totals.tokens().output()),
                                (L10nKey::StatisticsCacheRead, totals.tokens().cache_read()),
                                (L10nKey::StatisticsCacheWrite, totals.tokens().cache_write()),
                                (L10nKey::StatisticsReasoning, totals.tokens().reasoning()),
                            ] {
                                content = content.child(h_flex().gap_1().justify_between()
                                    .text_xs().text_color(tk.k5)
                                    .child(t(label)).child(number(value)));
                            }
                            if let Some(previous) = previous {
                                let change = totals.token_change(previous)
                                    .map(|v| format!("{v:+.1}%"))
                                    .unwrap_or_else(|| t(L10nKey::StatisticsNotComparable).into());
                                content = content.child(div().pt_1().text_xs().text_color(tk.k5)
                                    .child(t_fmt(L10nKey::StatisticsTokenDelta, &[("change", &change)])));
                            }
                        }
                        Metric::Cost => {
                            content = content.child(div().text_xs().text_color(tk.k5)
                                .child(t_fmt(L10nKey::StatisticsRecordedSubtotal,
                                    &[("count", &totals.unpriced_calls().to_string())])));
                            content = content.child(div().text_xs().text_color(tk.k5)
                                .child(t_fmt(L10nKey::StatisticsNativeEstimate,
                                    &[("value", &totals.estimated_usd().map(|v| format!("USD {v:.4}"))
                                        .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()))])));
                            for (currency, estimate) in totals.estimated_costs() {
                                let amount = |value: Option<f64>| value.map(|v| format!("{v:.4}"))
                                    .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into());
                                content = content.child(div().text_xs().text_color(tk.k5)
                                    .child(t_fmt(L10nKey::StatisticsCurrencyEstimate, &[
                                        ("currency", currency), ("custom", &amount(estimate.custom)),
                                        ("catalog", &amount(estimate.catalog)),
                                        ("partial", &estimate.partial_calls.to_string()),
                                    ])));
                            }
                            if let Some(previous) = previous {
                                content = content.child(div().mt_auto().pt_1().text_xs().text_color(tk.k5)
                                    .child(t_fmt(L10nKey::StatisticsPreviousValue,
                                        &[("value", &money(previous.recorded_usd()))])));
                            }
                        }
                        Metric::Sessions => {
                            let average = totals.average_tokens().map(|v| format!("{v:.1}"))
                                .unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into());
                            content = content.child(div().text_xs().text_color(tk.k5)
                                .child(t_fmt(L10nKey::StatisticsSessionAverage, &[("value", &average)])))
                                .child(div().text_xs().text_color(tk.k5)
                                    .child(t_fmt(L10nKey::StatisticsRecordCount, &[
                                        ("calls", &totals.calls().to_string()),
                                        ("cumulative", &totals.cumulative_records().to_string()),
                                    ])));
                            if let Some(previous) = previous {
                                content = content.child(div().mt_auto().pt_1().text_xs().text_color(tk.k5)
                                    .child(t_fmt(L10nKey::StatisticsPreviousValue,
                                        &[("value", &session_count(previous))])));
                            }
                        }
                    }
                }
                Button::new(id).ghost().h_auto().flex_1().min_w_0().min_h(rems(11.5))
                    .p_3().rounded_lg().bg(tk.k06).selected(metric == selected_metric)
                    .accessibility_label(t(key)).child(content)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_analysis(|s| s.metric = selected_metric, false, cx);
                    })).into_any_element()
            })).into_any_element()
    }
}
