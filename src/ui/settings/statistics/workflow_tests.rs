//! 实际 GPUI 入口完成来源关联与价格管理；原生文件仅来自临时目录。
use super::*;
use crate::ui::app::LyrelApp;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
mod test_host;

struct ControlledCatalog(std::sync::atomic::AtomicUsize);
impl storage::CatalogHttp for ControlledCatalog {
    fn get(&self, _: Option<&str>) -> Result<Vec<u8>, String> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Ok(br#"{"fixture-provider":{"id":"fixture-provider","models":{"fixture-model":{"id":"fixture-model","cost":{"input":4,"output":5}}}}}"#.to_vec())
        } else {
            Err("controlled offline".into())
        }
    }
}
fn settle(app: &Entity<SettingsWindow>, cx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if cx.update(|_, cx| {
            let state = &app.read(cx).active_settings().unwrap().statistics;
            !state.loading && !state.management.busy
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "statistics workflow did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
fn reveal(app: &Entity<SettingsWindow>, cx: &mut VisualTestContext, id: &str) {
    cx.update(|window, cx| {
        window.render_frame(cx);
        let top = window
            .find(SharedString::from(id.to_owned()))
            .bounds()
            .top();
        let state = &app.read(cx).active_settings().unwrap().statistics;
        let scroll = if state.management.surface.is_some() {
            &state.management.scroll
        } else {
            &state.content_scroll
        };
        let offset = scroll.offset();
        scroll.set_offset(gpui_kit::point(offset.x, offset.y - top + px(180.)));
        window.render_frame(cx);
        assert!(
            window.find(SharedString::from(id.to_owned())).visible(),
            "{id} must remain reachable"
        );
    });
}
fn click(app: &Entity<SettingsWindow>, cx: &mut VisualTestContext, id: &str) {
    reveal(app, cx, id);
    cx.update(|window, cx| window.click(SharedString::from(id.to_owned()), cx));
    cx.run_until_parked();
}
fn type_into(app: &Entity<SettingsWindow>, cx: &mut VisualTestContext, id: &str, value: &str) {
    click(app, cx, id);
    cx.update(|window, cx| {
        assert!(
            window
                .find(SharedString::from(id.to_owned()))
                .label()
                .is_some_and(|name| !name.is_empty()),
            "{id} must expose a semantic input name"
        );
        window.press("secondary-a", cx);
        window.input(value, cx);
    });
}

fn tab_to(cx: &mut VisualTestContext, id: &'static str) {
    for _ in 0..160 {
        if cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).focused() == Some(true)
        }) {
            return;
        }
        cx.update(|window, cx| window.press("tab", cx));
    }
    panic!("{id} must be reachable through real Tab navigation");
}

#[gpui_kit::test]
fn statistics_source_and_price_management_workflow(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("omp");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    let extra = temp.path().join("additional.jsonl");
    let timestamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(&extra, format!("{{\"type\":\"session\",\"version\":3,\"id\":\"managed-source\",\"timestamp\":\"{timestamp}\",\"cwd\":\"/isolated\"}}\n{{\"type\":\"model_usage\",\"id\":\"missing-cost\",\"timestamp\":\"{timestamp}\",\"provider\":\"fixture-provider\",\"model\":\"fixture-model\",\"usage\":{{\"input\":1000000,\"output\":2000000,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":3000000}}}}\n")).unwrap();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        // Exercise form interactions at rest, not the native dialog's entrance transition.
        cx.set_reduce_motion(true);
        let mut config = Config::default();
        config.agent_launch.insert(
            "omp".into(),
            format!("PI_CODING_AGENT_DIR='{}' omp", home.display()),
        );
        for agent in ["pi", "kimi", "opencode", "hermes"] {
            config.agent_launch.insert(
                agent.into(),
                format!("'{}/missing/{agent}'", temp.path().display()),
            );
        }
        cx.set_global(config);
        crate::ui::keymap::init(cx);
    });
    let window = cx.add_window(|window, cx| {
        let app = cx.new(|cx| {
            LyrelApp::with_session(
                None,
                Some(crate::core::session::Session::default()),
                window,
                cx,
            )
        });
        gpui_kit::component::Root::new(app, window, cx)
    });
    let workspace = window
        .update(cx, |root, _, _| {
            root.view().clone().downcast::<LyrelApp>().unwrap()
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::General, window, cx);
        let state = &mut app.active_settings_mut().unwrap().statistics;
        state.management.path = Some(temp.path().join("statistics-settings.json"));
        state.agent = Some("omp");
        state.period = Period::All;
        app.navigate_settings(SettingsSection::Statistics, None, window, cx);
    });
    settle(&app, &mut vcx);
    vcx.simulate_resize(gpui_kit::size(px(955.), px(701.)));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        for id in [
            "statistics-tile-tokens",
            "statistics-tile-cost",
            "statistics-tile-sessions",
            "statistics-analysis-metric",
        ] {
            assert!(
                window.find(id).visible(),
                "{id} must be visible in the first viewport"
            );
        }
    });
    click(&app, &mut vcx, "statistics-tile-cost");
    assert_eq!(
        app.read_with(&vcx, |app, _| app
            .active_settings()
            .unwrap()
            .statistics
            .analysis
            .metric),
        statistics::analysis::Metric::Cost
    );
    click(&app, &mut vcx, "statistics-tile-tokens");
    assert_eq!(
        app.read_with(&vcx, |app, _| app
            .active_settings()
            .unwrap()
            .statistics
            .analysis
            .metric),
        statistics::analysis::Metric::Tokens
    );
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.active_settings().unwrap().focus_handle, cx)
    });
    tab_to(&mut vcx, "statistics-machine");
    vcx.simulate_keystrokes("enter escape");
    vcx.run_until_parked();
    assert!(vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find("statistics-machine").focused() == Some(true)
    }));
    // Range is a workflow precondition, not a test of the framework's menu order.
    app.update_in(&mut vcx, |app, _, cx| {
        app.select_statistics_period(Period::Custom, cx)
    });
    settle(&app, &mut vcx);
    tab_to(&mut vcx, "statistics-apply");
    vcx.update(|window, cx| window.press("enter", cx));
    settle(&app, &mut vcx);
    tab_to(&mut vcx, "statistics-refresh");
    vcx.update(|window, cx| window.press("enter", cx));
    settle(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, cx| {
        app.select_statistics_period(Period::All, cx)
    });
    settle(&app, &mut vcx);
    reveal(&app, &mut vcx, "statistics-manage-sources");
    tab_to(&mut vcx, "statistics-manage-sources");
    reveal(&app, &mut vcx, "statistics-manage-sources");
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    click(&app, &mut vcx, "statistics-add-source");
    reveal(&app, &mut vcx, "statistics-source-kind");
    vcx.simulate_keystrokes("shift-tab");
    vcx.run_until_parked();
    assert!(vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find("statistics-source-kind").focused() == Some(true)
    }));
    vcx.simulate_keystrokes("enter down enter");
    vcx.run_until_parked();
    type_into(
        &app,
        &mut vcx,
        "statistics-source-selector",
        &extra.display().to_string(),
    );
    click(&app, &mut vcx, "statistics-source-save");
    settle(&app, &mut vcx);
    let binding = vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        assert_eq!(
            state.results[&(HostId::LOCAL, Some("omp"))]
                .totals
                .tokens()
                .total(),
            Some(3_000_000)
        );
        state.results[&(HostId::LOCAL, Some("omp"))]
            .bindings
            .iter()
            .find(|b| b.selector.contains("additional.jsonl"))
            .unwrap()
            .clone()
    });
    click(
        &app,
        &mut vcx,
        &format!("statistics-check-source:{}", binding.key()),
    );
    settle(&app, &mut vcx);
    click(&app, &mut vcx, "statistics-add-source");
    type_into(
        &app,
        &mut vcx,
        "statistics-source-selector",
        &extra.display().to_string(),
    );
    click(&app, &mut vcx, "statistics-source-save");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        assert!(state.management.error.is_some());
        assert_eq!(
            state.results[&(HostId::LOCAL, Some("omp"))]
                .totals
                .tokens()
                .total(),
            Some(3_000_000)
        );
    });
    click(&app, &mut vcx, "statistics-editor-cancel");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find("statistics-manage-sources").focused() == Some(true)
    }));
    reveal(&app, &mut vcx, "statistics-summary-details");
    tab_to(&mut vcx, "statistics-summary-details");
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    reveal(&app, &mut vcx, "statistics-summary-values");
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("statistics-summary-values")
                .label()
                .unwrap()
                .contains("3,000,000")
        );
        assert_eq!(
            window.find("statistics-token-composition").label(),
            Some(format!("{}: 3,000,000", t(L10nKey::StatisticsTokens)).as_str())
        );
        assert!(
            app.read(cx)
                .active_settings()
                .unwrap()
                .statistics
                .analysis
                .summary_detail
        );
    });
    click(&app, &mut vcx, "statistics-summary-details");
    assert!(!app.read_with(&vcx, |app, _| {
        app.active_settings()
            .unwrap()
            .statistics
            .analysis
            .summary_detail
    }));
    for (locale, width, mode) in [
        ("en", 1120., gpui_kit::component::ThemeMode::Light),
        ("zh-CN", 620., gpui_kit::component::ThemeMode::Dark),
    ] {
        crate::ui::i18n::set_locale(locale);
        vcx.simulate_resize(gpui_kit::size(px(width), px(800.)));
        app.update_in(&mut vcx, |_, window, cx| {
            gpui_kit::component::Theme::change(mode, Some(window), cx);
            gpui_kit::component::Theme::update(cx, |t| t.font_size = px(18.));
        });
        reveal(&app, &mut vcx, "statistics-manage-prices");
        tab_to(&mut vcx, "statistics-manage-prices");
        reveal(&app, &mut vcx, "statistics-manage-prices");
        vcx.update(|window, cx| window.press("enter", cx));
        vcx.run_until_parked();
        click(&app, &mut vcx, "statistics-add-price");
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("statistics-price-save").is_none());
            assert!(
                app.read(cx).active_settings().unwrap().navigation.section()
                    == SettingsSection::Statistics
            );
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(vcx.update(|window, cx| {
            window.render_frame(cx);
            window.find("statistics-manage-prices").focused() == Some(true)
        }));
    }
    crate::ui::i18n::set_locale("en");
    click(&app, &mut vcx, "statistics-manage-prices");
    click(&app, &mut vcx, "statistics-add-price");
    for (index, value) in [
        "fixture-provider",
        "fixture-model",
        "USD",
        "2",
        "3",
        "",
        "0",
        "0",
    ]
    .into_iter()
    .enumerate()
    {
        type_into(
            &app,
            &mut vcx,
            &format!("statistics-price-field-{index}"),
            value,
        );
    }
    click(&app, &mut vcx, "statistics-price-save");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        let totals = &state.results[&(HostId::LOCAL, Some("omp"))].totals;
        assert_eq!(totals.recorded_usd(), None);
        assert_eq!(totals.estimated_costs()["USD"].custom, Some(8.));
        let persisted =
            storage::StoredStatistics::load(&temp.path().join("statistics-settings.json")).unwrap();
        assert_eq!(persisted.pricing.custom[0].input, Some(2.));
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    click(&app, &mut vcx, "statistics-tile-cost");
    let priced_model = vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        let model = state.results[&(HostId::LOCAL, Some("omp"))]
            .analysis
            .as_ref()
            .unwrap()
            .models()
            .iter()
            .find(|model| model.label() == Some("fixture-model"))
            .unwrap();
        format!(
            "statistics-breakdown:false:{}:{}:{}:fixture-model",
            model.agent(),
            model.scope(),
            model.provider().unwrap_or("")
        )
    });
    reveal(&app, &mut vcx, &priced_model);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(SharedString::from(priced_model))
                .label()
                .unwrap()
                .contains("$8.00"),
            "the model row must include the explicit USD estimate when no native bill exists"
        );
    });
    click(&app, &mut vcx, "statistics-manage-prices");
    click(
        &app,
        &mut vcx,
        "statistics-edit-price:fixture-provider:fixture-model:USD",
    );
    type_into(&app, &mut vcx, "statistics-price-field-3", "4");
    click(&app, &mut vcx, "statistics-price-save");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        assert_eq!(
            app.read(cx).active_settings().unwrap().statistics.results
                [&(HostId::LOCAL, Some("omp"))]
                .totals
                .estimated_costs()["USD"]
                .custom,
            Some(10.)
        )
    });
    let http = Arc::new(ControlledCatalog(std::sync::atomic::AtomicUsize::new(0)));
    app.update_in(&mut vcx, |app, _, _| {
        app.active_settings_mut()
            .unwrap()
            .statistics
            .management
            .http = http.clone()
    });
    click(&app, &mut vcx, "statistics-catalog-enable");
    settle(&app, &mut vcx);
    // 启用本身不请求网络；真实下载仅由显式手动检查触发。
    vcx.update(|_, cx| {
        assert_eq!(
            app.read(cx)
                .active_settings()
                .unwrap()
                .statistics
                .management
                .stored
                .pricing
                .checked_at,
            None
        )
    });
    assert_eq!(http.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    click(&app, &mut vcx, "statistics-catalog-check");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        assert_eq!(state.management.stored.pricing.catalog[0].input, Some(4.));
        assert_eq!(
            state.results[&(HostId::LOCAL, Some("omp"))]
                .totals
                .estimated_costs()["USD"]
                .custom,
            Some(10.)
        );
    });
    click(&app, &mut vcx, "statistics-catalog-check");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let pricing = &app
            .read(cx)
            .active_settings()
            .unwrap()
            .statistics
            .management
            .stored
            .pricing;
        assert_eq!(pricing.catalog[0].input, Some(4.));
        assert_eq!(pricing.last_error.as_deref(), Some("controlled offline"));
        assert!(pricing.fetched_at.is_some());
    });
    click(&app, &mut vcx, "statistics-catalog-enable");
    settle(&app, &mut vcx);
    click(
        &app,
        &mut vcx,
        "statistics-delete-price:fixture-provider:fixture-model:USD",
    );
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        assert!(
            app.read(cx).active_settings().unwrap().statistics.results
                [&(HostId::LOCAL, Some("omp"))]
                .totals
                .estimated_costs()
                .is_empty()
        )
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    click(&app, &mut vcx, "statistics-manage-sources");
    click(
        &app,
        &mut vcx,
        &format!("statistics-remove-source:{}", binding.key()),
    );
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        assert_eq!(
            state.results[&(HostId::LOCAL, Some("omp"))].totals.calls(),
            0
        );
        assert!(
            extra.exists(),
            "association removal must not delete history"
        );
    });
    app.update_in(&mut vcx, |app, window, cx| app.close_settings(window, cx));
}

#[gpui_kit::test]
fn statistics_refresh_discovery_failure_keeps_visible_totals_and_original_time(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let agent = temp.path().join("pi");
    let history = temp.path().join("history");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir(&history).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        serde_json::to_vec(&serde_json::json!({"sessionDir":history})).unwrap(),
    )
    .unwrap();
    std::fs::write(history.join("visible.jsonl"), "{\"type\":\"session\",\"version\":3,\"id\":\"visible-cache\",\"timestamp\":\"2026-10-19T00:00:00Z\"}\n{\"type\":\"message\",\"id\":\"usage\",\"timestamp\":\"2026-10-19T00:00:01Z\",\"message\":{\"role\":\"assistant\",\"provider\":\"fixture\",\"model\":\"fixture\",\"usage\":{\"input\":40,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0}}}}\n").unwrap();
    let (app, mut vcx) = crate::ui::settings::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        cx.global_mut::<Config>().agent_launch.insert(
            "pi".into(),
            format!("PI_CODING_AGENT_DIR='{}' pi", agent.display()),
        );
        app.open_settings_section(SettingsSection::General, window, cx);
        let state = &mut app.active_settings_mut().unwrap().statistics;
        state.management.path = Some(temp.path().join("settings.json"));
        state.agent = Some("pi");
        state.period = Period::All;
        app.navigate_settings(SettingsSection::Statistics, None, window, cx);
    });
    settle(&app, &mut vcx);
    let observed = app.read_with(&vcx, |app, _| {
        let visible =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(visible.totals.tokens().total(), Some(42));
        visible.snapshot.updated()
    });
    reveal(&app, &mut vcx, "statistics-cost-trend");
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_ne!(
            window.find("statistics-cost-trend").label(),
            Some(t(L10nKey::StatisticsTrendUnavailable)),
            "complete native zero costs are real known data, not an unavailable trend"
        );
        assert_eq!(
            window.find("statistics-token-composition").label(),
            Some(format!("{}: 42", t(L10nKey::StatisticsTokens)).as_str())
        );
    });
    let rank_id = app.read_with(&vcx, |app, _| {
        let group = &app.active_settings().unwrap().statistics.results
            [&(HostId::LOCAL, Some("pi"))]
            .analysis
            .as_ref()
            .unwrap()
            .models()[0];
        format!(
            "statistics-breakdown:false:{}:{}:{}:{}",
            group.agent(),
            group.scope(),
            group.provider().unwrap_or(""),
            group.label().unwrap()
        )
    });
    for (tile, expected_value) in [
        ("statistics-tile-cost", "$0.00"),
        ("statistics-tile-sessions", "1"),
        ("statistics-tile-tokens", "42"),
    ] {
        click(&app, &mut vcx, tile);
        reveal(&app, &mut vcx, &rank_id);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let row = window.find(rank_id.clone());
            assert!(
                row.label().unwrap().lines().next().unwrap().ends_with(expected_value),
                "the ranking's primary value must follow the selected metric, including native zero cost"
            );
        });
    }
    std::fs::write(agent.join("settings.json"), "unreadable native settings").unwrap();
    click(&app, &mut vcx, "statistics-refresh");
    settle(&app, &mut vcx);
    app.read_with(&vcx, |app, _| {
        let visible =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(visible.totals.tokens().total(), Some(42));
        assert_eq!(visible.totals.main_sessions(), 1);
        assert!(!visible.totals.is_complete());
        assert_eq!(visible.snapshot.updated(), observed);
        assert_eq!(
            visible.analysis.as_ref().unwrap().totals().tokens().total(),
            Some(42)
        );
    });
    reveal(&app, &mut vcx, "statistics-cost-trend");
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("statistics-cost-trend").label(),
            Some(t(L10nKey::StatisticsTrendUnavailable)),
            "retained partial values must not produce a complete-looking daily curve"
        );
        assert_eq!(
            window.find("statistics-session-trend").label(),
            Some(t(L10nKey::StatisticsTrendUnavailable))
        );
    });
}

fn cached_filter_fixture(
    cx: &mut TestAppContext,
    block_scan: bool,
) -> (
    tempfile::TempDir,
    Arc<test_host::CountingHost>,
    Option<test_host::ScanGate>,
    Entity<SettingsWindow>,
    VisualTestContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("pi");
    let history = home.join("sessions");
    std::fs::create_dir_all(&history).unwrap();
    std::fs::write(history.join("visible.jsonl"), "{\"type\":\"session\",\"version\":3,\"id\":\"cached-filter\",\"timestamp\":\"2026-10-18T12:00:00Z\"}\n{\"type\":\"message\",\"id\":\"earlier\",\"timestamp\":\"2026-10-18T12:00:01Z\",\"message\":{\"role\":\"assistant\",\"provider\":\"fixture\",\"model\":\"fixture\",\"usage\":{\"input\":40,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0}}}}\n{\"type\":\"message\",\"id\":\"later\",\"timestamp\":\"2026-10-19T12:00:01Z\",\"message\":{\"role\":\"assistant\",\"provider\":\"fixture\",\"model\":\"fixture\",\"usage\":{\"input\":60,\"output\":10,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0}}}}\n").unwrap();
    let host = test_host::CountingHost::new(temp.path());
    let gate = block_scan.then(|| host.block_scan());
    let (app, mut vcx) = crate::ui::settings::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        HostRegistry::insert(cx, host.clone());
        cx.global_mut::<Config>().agent_launch.insert(
            "pi".into(),
            format!("PI_CODING_AGENT_DIR='{}' pi", home.display()),
        );
        app.open_settings_section(SettingsSection::General, window, cx);
        let state = &mut app.active_settings_mut().unwrap().statistics;
        state.management.path = Some(temp.path().join("statistics-settings.json"));
        state.agent = Some("pi");
        state.period = Period::All;
        app.navigate_settings(SettingsSection::Statistics, None, window, cx);
    });
    (temp, host, gate, app, vcx)
}

#[gpui_kit::test]
fn statistics_cached_filters_recompute_without_host_reads_and_refresh_reads_changes(
    cx: &mut TestAppContext,
) {
    let (temp, host, _, app, mut vcx) = cached_filter_fixture(cx, false);
    settle(&app, &mut vcx);
    let reads = host.reads();
    assert!(reads > 0);
    let updated = app.read_with(&vcx, |app, _| {
        let presentation =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(presentation.totals.tokens().total(), Some(112));
        presentation.snapshot.updated()
    });
    app.update_in(&mut vcx, |app, _, cx| {
        app.select_statistics_period(Period::Custom, cx)
    });
    settle(&app, &mut vcx);
    type_into(&app, &mut vcx, "statistics-start-date", "2026-10-19");
    type_into(&app, &mut vcx, "statistics-end-date", "2026-10-19");
    click(&app, &mut vcx, "statistics-apply");
    settle(&app, &mut vcx);
    app.read_with(&vcx, |app, _| {
        let presentation =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(presentation.totals.tokens().total(), Some(70));
        assert_eq!(
            presentation.previous.as_ref().unwrap().tokens().total(),
            Some(42)
        );
        assert_eq!(
            presentation
                .analysis
                .as_ref()
                .unwrap()
                .totals()
                .tokens()
                .total(),
            Some(70)
        );
        assert_eq!(presentation.snapshot.updated(), updated);
    });
    let year = chrono::Datelike::year(&chrono::Local::now());
    click(&app, &mut vcx, "statistics-year-previous");
    settle(&app, &mut vcx);
    app.read_with(&vcx, |app, _| {
        let presentation =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(presentation.analysis.as_ref().unwrap().year(), year - 1);
        assert_eq!(presentation.totals.tokens().total(), Some(70));
    });
    app.update_in(&mut vcx, |app, _, cx| {
        app.change_analysis(|analysis| analysis.agent = Some("omp".into()), true, cx)
    });
    settle(&app, &mut vcx);
    app.read_with(&vcx, |app, _| {
        let presentation =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(presentation.totals.tokens().total(), Some(70));
        assert_eq!(presentation.analysis.as_ref().unwrap().totals().calls(), 0);
    });
    assert_eq!(
        host.reads(),
        reads,
        "view filters must not rediscover or reread the Host"
    );
    let file = temp.path().join("pi/sessions/visible.jsonl");
    use std::io::Write as _;
    writeln!(std::fs::OpenOptions::new().append(true).open(file).unwrap(), "{{\"type\":\"message\",\"id\":\"new-usage\",\"timestamp\":\"2026-10-19T12:00:02Z\",\"message\":{{\"role\":\"assistant\",\"provider\":\"fixture\",\"model\":\"fixture\",\"usage\":{{\"input\":100,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{{\"total\":0}}}}}}}}").unwrap();
    click(&app, &mut vcx, "statistics-refresh");
    settle(&app, &mut vcx);
    assert!(
        host.reads() > reads,
        "manual refresh must read the current Host"
    );
    app.read_with(&vcx, |app, _| {
        let presentation =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(presentation.totals.tokens().total(), Some(171));
        assert_eq!(
            presentation.previous.as_ref().unwrap().tokens().total(),
            Some(42)
        );
    });
}

#[gpui_kit::test]
fn statistics_filter_changes_during_scan_do_not_queue_another_scan(cx: &mut TestAppContext) {
    let (_temp, host, gate, app, mut vcx) = cached_filter_fixture(cx, true);
    let gate = gate.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if gate.entered.try_recv().is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native scan did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let reads = host.reads();
    let listings = host.listings();
    assert_eq!(listings, 1, "the fixture has one native session directory");
    app.update_in(&mut vcx, |app, window, cx| {
        app.select_statistics_period(Period::Year, cx);
        app.select_statistics_period(Period::Month, cx);
        app.select_statistics_period(Period::Custom, cx);
        let state = &app.active_settings().unwrap().statistics;
        state
            .start
            .clone()
            .update(cx, |input, cx| input.set_value("2026-10-19", window, cx));
        state
            .end
            .clone()
            .update(cx, |input, cx| input.set_value("2026-10-19", window, cx));
    });
    click(&app, &mut vcx, "statistics-apply");
    vcx.run_until_parked();
    assert_eq!(
        host.reads(),
        reads,
        "changing a date must not queue discovery behind the scan"
    );
    gate.release.send_blocking(()).unwrap();
    settle(&app, &mut vcx);
    assert_eq!(
        host.listings(),
        listings,
        "the pending scan must be the only native discovery pass"
    );
    app.read_with(&vcx, |app, _| {
        let presentation =
            &app.active_settings().unwrap().statistics.results[&(HostId::LOCAL, Some("pi"))];
        assert_eq!(presentation.totals.tokens().total(), Some(70));
        assert_eq!(
            presentation.previous.as_ref().unwrap().tokens().total(),
            Some(42)
        );
        assert_eq!(
            presentation
                .analysis
                .as_ref()
                .unwrap()
                .totals()
                .tokens()
                .total(),
            Some(70)
        );
    });
}
