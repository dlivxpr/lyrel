//! 来源关联和单价编辑属于统计页，不引入另一条扫描或聚合管线。
use super::*;
use lyrel_core::statistics::{pricing::UnitPrice, sources::SourceBinding};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq)]
enum Editor { Source, Price }
pub(super) struct Management {
    pub stored: storage::StoredStatistics,
    pub loaded: bool,
    pub busy: bool,
    pub error: Option<String>,
    status: Option<String>,
    pub(super) path: Option<PathBuf>,
    editor: Option<Editor>,
    source_agent: &'static str,
    selector: Entity<InputState>,
    price: Vec<Entity<InputState>>,
    return_focus: Option<FocusHandle>,
    revision: Uuid,
    pub(super) http: Arc<dyn storage::CatalogHttp>,
    editing_price: Option<(String, String, String)>,
    editor_host: HostId,
    editor_window: Option<gpui_kit::AnyWindowHandle>,
}
impl Management {
    pub fn reset_scope(&mut self) { self.error = None; self.status = None; }
    pub fn new(window: &mut Window, cx: &mut Context<LyrelApp>) -> Self {
        Self { stored: Default::default(), loaded: false, busy: false, error: None, status: None,
            path: crate::core::config::config_path("statistics-settings.json"), editor: None,
            source_agent: "omp", selector: cx.new(|cx| InputState::new(window, cx)),
            price: (0..8).map(|_| cx.new(|cx| InputState::new(window, cx))).collect(),
            return_focus: None, revision: Uuid::new_v4(), http: Arc::new(storage::PublicCatalogHttp), editing_price: None, editor_host: HostId::LOCAL, editor_window: None }
    }
}
impl LyrelApp {
    pub(super) fn ensure_statistics_settings(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(settings) = self.active_settings_mut() else { return false; };
        let state = &mut settings.statistics.management;
        if state.loaded { return true; }
        if state.busy || state.error.is_some() { return false; }
        state.busy = true;
        settings.statistics.loading = true;
        let path = state.path.clone();
        let revision = state.revision;
        let Some(host) = HostRegistry::lookup(cx, HostId::LOCAL) else { return false; };
        HostOps::run(host, cx, move |_| {
            storage::StoredStatistics::load(&path.ok_or_else(|| std::io::Error::other("Statistics configuration directory unavailable"))?)
        }, move |this, result, cx| {
            let Some(settings) = this.active_settings_mut() else { return; };
            if settings.statistics.management.revision != revision { return; }
            settings.statistics.loading = false;
            let state = &mut settings.statistics.management;
            state.busy = false;
            match result { Ok(stored) => { state.stored = stored; state.loaded = true; }, Err(error) => state.error = Some(error.to_string()) }
            if state.loaded { this.load_statistics(cx); }
            cx.notify();
        });
        false
    }
    fn edit_statistics(&mut self, editor: Editor, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.active_settings_mut() else { return; };
        let state = &mut settings.statistics.management;
        state.editor_host = settings.statistics.host;
        if state.busy || !state.loaded { return; }
        state.editor = Some(editor);
        state.return_focus = window.focused(cx);
        state.editor_window = Some(window.window_handle());
        state.error = None;
        if editor == Editor::Price {
            state.editing_price = None;
            for (index, input) in state.price.iter().enumerate() { input.update(cx, |s,cx| s.set_value(if index == 2 { "USD" } else { "" }, window, cx)); }
        }
        let input = if editor == Editor::Source { &state.selector } else { &state.price[0] };
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }
    pub(crate) fn cancel_statistics_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.active_settings_mut().map(|s| &mut s.statistics.management) else { return false; };
        if state.editor.take().is_none() { return false; }
        if let Some(focus) = state.return_focus.take() { window.focus(&focus, cx); }
        cx.notify(); true
    }
    fn change_statistics_settings(&mut self, mutation: impl FnOnce(&mut storage::StoredStatistics, &dyn lyrel_core::host::Host) -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.change_statistics_settings_on(HostId::LOCAL, None, mutation, cx);
    }
    fn change_statistics_settings_on(&mut self, host_id: HostId, source_host: Option<HostId>, mutation: impl FnOnce(&mut storage::StoredStatistics, &dyn lyrel_core::host::Host) -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        let Some(settings) = self.active_settings_mut() else { return; };
        let state = &mut settings.statistics.management;
        if state.busy || !state.loaded { return; }
        let mut stored = state.stored.clone();
        let path = state.path.clone();
        let revision = Uuid::new_v4(); state.revision = revision; state.busy = true; state.error = None;
        let mut results = settings.statistics.results.clone();
        let analysis_agent = settings.statistics.analysis.agent.clone();
        let year = settings.statistics.analysis.year;
        settings.statistics.invalidate();
        let Some(host) = HostRegistry::lookup(cx, host_id) else {
            if let Some(s) = self.active_settings_mut() { s.statistics.management.busy = false; s.statistics.management.error = Some(t(L10nKey::StatisticsOffline).into()); }
            cx.notify(); return;
        };
        HostOps::run(host, cx, move |host| -> Result<_, String> {
            mutation(&mut stored, host)?;
            stored.save(&path.ok_or("Statistics configuration directory unavailable")?).map_err(|e| e.to_string())?;
            if let Some(host) = source_host {
                results.retain(|(id,_),_| *id != host);
            } else {
                for presentation in results.values_mut() {
                    presentation.snapshot.apply_pricing(stored.pricing.clone());
                    presentation.totals = presentation.snapshot.aggregate(&presentation.window);
                    presentation.previous = presentation.window.previous().map(|window| presentation.snapshot.aggregate(window));
                    match presentation.snapshot.analyze(&presentation.window,chrono::Local::now(),analysis_agent.as_deref(),year) {
                        Ok(analysis) => { presentation.analysis = Some(analysis); presentation.analysis_error = None; },
                        Err(error) => { presentation.analysis = None; presentation.analysis_error = Some(error); }
                    }
                }
            }
            Ok((stored,results))
        }, move |this, result, cx| {
            let Some(settings) = this.active_settings_mut() else { return; };
            let state = &mut settings.statistics.management;
            if state.revision != revision { return; }
            state.busy = false;
            match result {
                Ok((stored,results)) => {
                    state.stored = stored;
                    state.editor = None;
                    let restore = state.editor_window.take().zip(state.return_focus.take());
                    settings.statistics.invalidate();
                    settings.statistics.results = results;
                    if let Some((handle, focus)) = restore { let _ = handle.update(cx, |_,window,cx| window.focus(&focus,cx)); }
                },
                Err(error) => state.error = Some(error),
            }
            this.load_statistics(cx); cx.notify();
        });
        cx.notify();
    }
    fn save_statistics_source(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.active_settings().map(|s| &s.statistics.management) else { return; };
        let selector = state.selector.read(cx).value().to_string();
        let agent = state.source_agent;
        let launch = cx.global::<Config>().agent_launch.clone();
        let host = state.editor_host;
        self.change_statistics_settings_on(host, Some(host), move |stored, host| stored.sources.add(host, agent, &selector, &launch).map(|_| ()).map_err(|e| e.to_string()), cx);
    }
    fn save_statistics_price(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.active_settings().map(|s| &s.statistics.management) else { return; };
        let fields: Vec<_> = state.price.iter().map(|input| input.read(cx).value().to_string()).collect();
        let previous = state.editing_price.clone();
        self.change_statistics_settings(move |stored, _| {
            let mut values = Vec::new();
            for field in &fields[3..] { values.push(if field.trim().is_empty() { None } else { Some(field.trim().parse::<f64>().map_err(|_| "Invalid token price")?) }); }
            let mut price = UnitPrice::default();
            price.provider = fields[0].trim().into(); price.model = fields[1].trim().into(); price.currency = fields[2].trim().into();
            price.input = values[0]; price.output = values[1]; price.reasoning = values[2]; price.cache_read = values[3]; price.cache_write = values[4];
            stored.pricing.upsert_custom(price)?;
            if let Some((provider, model, currency)) = previous {
                if (provider.as_str(), model.as_str(), currency.as_str()) != (fields[0].trim(), fields[1].trim(), fields[2].trim()) { stored.pricing.remove_custom(&provider,&model,&currency); }
            }
            Ok(())
        }, cx);
    }
    fn check_statistics_source(&mut self, binding: SourceBinding, cx: &mut Context<Self>) {
        let launch = cx.global::<Config>().agent_launch.clone();
        let Some(settings) = self.active_settings_mut() else { return; };
        let state = &mut settings.statistics.management;
        if state.busy { return; }
        state.busy = true; state.error = None; state.status = None;
        let revision = Uuid::new_v4(); state.revision = revision;
        let checked_host = HostId(binding.host);
        let stored = state.stored.clone();
        let Some(host) = HostRegistry::lookup(cx, HostId(binding.host)) else { if let Some(s) = self.active_settings_mut() { s.statistics.management.busy = false; s.statistics.management.error = Some(t(L10nKey::StatisticsOffline).into()); } cx.notify(); return; };
        HostOps::run(host, cx, move |host| -> std::io::Result<_> {
            stored.sources.check(host, &binding, &launch)?;
            Ok(())
        }, move |this, result, cx| {
            if let Some(s) = this.active_settings_mut() {
                if s.statistics.management.revision != revision { return; }
                s.statistics.management.busy = false;
                if s.statistics.host == checked_host {
                    match result {
                        Ok(()) => s.statistics.management.status = Some(t(L10nKey::StatisticsSourceChecked).into()),
                        Err(error) => s.statistics.management.error = Some(error.to_string()),
                    }
                }
            }
            this.load_statistics(cx); cx.notify();
        }); cx.notify();
    }
    fn check_statistics_catalog(&mut self, cx: &mut Context<Self>) {
        let proxy = cx.global::<Config>().http_proxy.clone();
        let Some(http) = self.active_settings().map(|s| s.statistics.management.http.clone()) else { return; };
        self.change_statistics_settings(move |stored, _| {
            // 即使下载失败，也持久化检查时间、错误和原有缓存。
            let _ = storage::check_catalog(&mut stored.pricing, &*http, proxy.as_deref(), chrono::Utc::now().timestamp());
            Ok(())
        }, cx);
    }
    pub(super) fn render_statistics_management(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(settings) = self.active_settings() else { return div().into_any_element(); };
        let state = &settings.statistics.management;
        let tk = Tk::of(cx);
        let disabled = !state.loaded || state.busy;
        let mut rows = vec![div().text_color(tk.k5).child(t(L10nKey::StatisticsSourcesTitle)).into_any_element(),
            div().text_sm().text_color(tk.k5).child(t(L10nKey::StatisticsSourcesHelp)).into_any_element(),
            self.settings_button("statistics-add-source", t(L10nKey::StatisticsAddSource), cx, |this, window, cx| this.edit_statistics(Editor::Source, window, cx)).disabled(disabled).into_any_element()];
        if let Some(presentation) = settings.statistics.results.get(&(settings.statistics.host, settings.statistics.agent)) {
            for binding in &presentation.bindings {
                let remove = binding.clone(); let check = binding.clone(); let id = binding.key();
                rows.push(v_flex().gap_2().min_w_0()
                    .child(div().text_sm().child(format!("{} · {}\n{}\n{}", binding.agent, binding.host, binding.scope, binding.selector)))
                    .child(h_flex().flex_wrap().gap_2()
                        .child(self.settings_button(SharedString::from(format!("statistics-check-source:{id}")), t(L10nKey::StatisticsCheckSource), cx, move |this, _, cx| this.check_statistics_source(check.clone(), cx)).disabled(disabled))
                        .child(self.settings_button(SharedString::from(format!("statistics-remove-source:{id}")), t(L10nKey::StatisticsRemoveSource), cx, move |this, _, cx| { let binding = remove.clone(); this.change_statistics_settings_on(HostId::LOCAL, Some(HostId(binding.host)), move |stored, _| { stored.sources.remove(&binding); Ok(()) }, cx); }).disabled(disabled)))
                    .into_any_element());
            }
        }
        if !state.loaded && state.error.is_some() {
            rows.push(self.settings_button("statistics-settings-retry", t(L10nKey::StatisticsRefresh), cx, |this,_,cx| { if let Some(s) = this.active_settings_mut() { s.statistics.management.error = None; } this.load_statistics(cx); }).disabled(state.busy).into_any_element());
        }
        if state.editor == Some(Editor::Source) {
            let entries = ["omp", "pi", "kimi", "opencode", "hermes"].into_iter().map(|agent| MenuEntry::item(agent, state.source_agent == agent, move |this, _, cx| { if let Some(s) = this.active_settings_mut() { s.statistics.management.source_agent = agent; } cx.notify(); })).collect();
            rows.push(self.settings_dropdown("statistics-source-kind", state.source_agent, None, entries, cx));
            rows.push(div().text_sm().child(t(L10nKey::StatisticsSourceSelector)).into_any_element());
            rows.push(div().text_sm().text_color(tk.k5).child(format!("{}: {}", t(L10nKey::StatisticsTitle), state.editor_host.0)).into_any_element());
            rows.push(Input::new(&state.selector).id("statistics-source-selector").w_full().into_any_element());
            rows.push(h_flex().flex_wrap().gap_2().child(self.settings_button("statistics-source-save", t(L10nKey::StatisticsAddSource), cx, |this, _, cx| this.save_statistics_source(cx)).disabled(disabled)).child(self.settings_button("statistics-editor-cancel", t(L10nKey::Cancel), cx, |this, window, cx| { this.cancel_statistics_editor(window, cx); })).into_any_element());
        }
        rows.push(div().text_color(tk.k5).child(t(L10nKey::StatisticsPricesTitle)).into_any_element());
        rows.push(div().text_sm().text_color(tk.k5).child(t(L10nKey::StatisticsPricesHelp)).into_any_element());
        rows.push(self.settings_button("statistics-add-price", t(L10nKey::StatisticsEditPrice), cx, |this, window, cx| this.edit_statistics(Editor::Price, window, cx)).disabled(disabled).into_any_element());
        for price in &state.stored.pricing.custom {
            let price = price.clone(); let remove = price.clone(); let edit = price.clone();
            let key = format!("{}:{}:{}", price.provider, price.model, price.currency);
            rows.push(v_flex().gap_2().child(format!("{} / {} · {}", price.provider, price.model, price.currency))
                .child(h_flex().flex_wrap().gap_2()
                    .child(self.settings_button(SharedString::from(format!("statistics-edit-price:{key}")), t(L10nKey::StatisticsEditPrice), cx, move |this, window, cx| {
                        this.edit_statistics(Editor::Price, window, cx);
                        if let Some(s) = this.active_settings_mut() {
                            s.statistics.management.editing_price = Some((edit.provider.clone(), edit.model.clone(), edit.currency.clone()));
                            let values = [edit.provider.clone(), edit.model.clone(), edit.currency.clone(), edit.input.map(|v|v.to_string()).unwrap_or_default(), edit.output.map(|v|v.to_string()).unwrap_or_default(), edit.reasoning.map(|v|v.to_string()).unwrap_or_default(), edit.cache_read.map(|v|v.to_string()).unwrap_or_default(), edit.cache_write.map(|v|v.to_string()).unwrap_or_default()];
                            for (input, value) in s.statistics.management.price.iter().zip(values) { input.update(cx, |s,cx| s.set_value(value, window, cx)); }
                        }
                    }).disabled(disabled))
                    .child(self.settings_button(SharedString::from(format!("statistics-delete-price:{key}")), t(L10nKey::StatisticsDeletePrice), cx, move |this, _, cx| { let p = remove.clone(); this.change_statistics_settings(move |stored,_| { stored.pricing.remove_custom(&p.provider,&p.model,&p.currency); Ok(()) },cx); }).disabled(disabled))).into_any_element());
        }
        if state.editor == Some(Editor::Price) {
            for (index, key) in [L10nKey::StatisticsPriceProvider,L10nKey::StatisticsPriceModel,L10nKey::StatisticsPriceCurrency,L10nKey::StatisticsInput,L10nKey::StatisticsOutput,L10nKey::StatisticsReasoning,L10nKey::StatisticsCacheRead,L10nKey::StatisticsCacheWrite].into_iter().enumerate() {
                rows.push(v_flex().gap_1().child(t(key)).child(Input::new(&state.price[index]).id(SharedString::from(format!("statistics-price-field-{index}"))).w_full()).into_any_element());
            }
            rows.push(h_flex().flex_wrap().gap_2().child(self.settings_button("statistics-price-save", t(L10nKey::StatisticsSavePrice), cx, |this,_,cx| this.save_statistics_price(cx)).disabled(disabled)).child(self.settings_button("statistics-editor-cancel",t(L10nKey::Cancel),cx,|this,w,cx|{this.cancel_statistics_editor(w,cx);})).into_any_element());
        }
        let enabled = state.stored.pricing.public_enabled;
        rows.push(self.settings_row(t(L10nKey::StatisticsPublicCatalog),t(L10nKey::StatisticsPublicConsent),self.settings_button("statistics-catalog-enable",t(if enabled {L10nKey::StatisticsCatalogDisable}else{L10nKey::StatisticsCatalogEnable}),cx,move |this,_,cx| this.change_statistics_settings(move |stored,_| { stored.pricing.public_enabled = !enabled; Ok(()) },cx)).disabled(disabled),cx).into_any_element());
        rows.push(self.settings_button("statistics-catalog-check",t(L10nKey::StatisticsCatalogCheck),cx,|this,_,cx| this.check_statistics_catalog(cx)).disabled(disabled || !enabled).into_any_element());
        rows.push(div().text_sm().text_color(tk.k5).child(t_fmt(L10nKey::StatisticsCatalogStatus,&[("checked",&format_time(state.stored.pricing.checked_at)),("fetched",&format_time(state.stored.pricing.fetched_at)),("count",&state.stored.pricing.catalog.len().to_string())])).into_any_element());
        if state.busy { rows.push(div().child(t(L10nKey::StatisticsLoading)).into_any_element()); }
        if let Some(status) = &state.status { rows.push(div().text_color(tk.k5).child(status.clone()).into_any_element()); }
        for error in [&state.error, &state.stored.pricing.last_error].into_iter().flatten() { rows.push(div().text_color(tk.warn_text).child(error.clone()).into_any_element()); }
        v_flex().gap_3().min_w_0().children(rows).into_any_element()
    }
}
fn format_time(timestamp: Option<i64>) -> String { timestamp.and_then(|t| chrono::DateTime::from_timestamp(t,0)).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S %:z").to_string()).unwrap_or_else(|| t(L10nKey::StatisticsUnknown).into()) }
