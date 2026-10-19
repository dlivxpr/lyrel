//! 实际 GPUI 入口完成来源关联与价格管理；原生文件仅来自临时目录。
use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

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
fn settle(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
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
fn reveal(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, id: &str) {
    cx.update(|window, cx| {
        window.render_frame(cx);
        let top = window
            .find(SharedString::from(id.to_owned()))
            .bounds()
            .top();
        let state = &app.read(cx).active_settings().unwrap().statistics;
        let scroll = if state.management.surface.is_some() { &state.management.scroll } else { &state.content_scroll };
        let offset = scroll.offset();
        scroll.set_offset(gpui_kit::point(offset.x, offset.y - top + px(180.)));
        window.render_frame(cx);
        assert!(
            window.find(SharedString::from(id.to_owned())).visible(),
            "{id} must remain reachable"
        );
    });
}
fn click(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, id: &str) {
    reveal(app, cx, id);
    cx.update(|window, cx| window.click(SharedString::from(id.to_owned()), cx));
    cx.run_until_parked();
}
fn type_into(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, id: &str, value: &str) {
    click(app, cx, id);
    cx.update(|window, cx| {
        assert!(window.find(SharedString::from(id.to_owned())).label().is_some_and(|name| !name.is_empty()),
            "{id} must expose a semantic input name");
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
    let app = window
        .update(cx, |root, _, _| {
            root.view().clone().downcast::<LyrelApp>().unwrap()
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::General, window, cx);
        let state = &mut app.active_settings_mut().unwrap().statistics;
        state.management.path = Some(temp.path().join("statistics-settings.json"));
        state.agent = Some("omp");
        state.period = Period::All;
        app.navigate_settings(SettingsSection::Statistics, None, window, cx);
    });
    settle(&app, &mut vcx);
    vcx.simulate_window_resize(window.into(), gpui_kit::size(px(955.), px(701.)));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        for id in ["statistics-tile-tokens", "statistics-tile-cost", "statistics-tile-sessions", "statistics-analysis-metric"] {
            assert!(window.find(id).visible(), "{id} must be visible in the first viewport");
        }
    });
    click(&app, &mut vcx, "statistics-tile-cost");
    assert_eq!(app.read_with(&vcx, |app, _| app.active_settings().unwrap().statistics.analysis.metric), statistics::analysis::Metric::Cost);
    click(&app, &mut vcx, "statistics-tile-tokens");
    assert_eq!(app.read_with(&vcx, |app, _| app.active_settings().unwrap().statistics.analysis.metric), statistics::analysis::Metric::Tokens);
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
    tab_to(&mut vcx, "statistics-agent");
    vcx.simulate_keystrokes("enter down down enter");
    settle(&app, &mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| app
            .active_settings()
            .unwrap()
            .statistics
            .agent),
        Some("omp")
    );
    tab_to(&mut vcx, "statistics-period");
    vcx.simulate_keystrokes("enter down down down down down down enter");
    settle(&app, &mut vcx);
    assert!(app.read_with(
        &vcx,
        |app, _| app.active_settings().unwrap().statistics.period == Period::Custom
    ));
    tab_to(&mut vcx, "statistics-apply");
    vcx.simulate_keystrokes("enter");
    settle(&app, &mut vcx);
    tab_to(&mut vcx, "statistics-refresh");
    vcx.simulate_keystrokes("enter");
    settle(&app, &mut vcx);
    reveal(&app, &mut vcx, "statistics-period");
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.active_settings().unwrap().focus_handle, cx)
    });
    tab_to(&mut vcx, "statistics-period");
    vcx.simulate_keystrokes("enter down down down down down enter");
    settle(&app, &mut vcx);
    click(&app, &mut vcx, "statistics-manage-sources");
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
    click(&app, &mut vcx, &format!("statistics-edit-source:{}", binding.key()));
    type_into(&app, &mut vcx, "statistics-source-selector", &temp.path().join("missing-history.jsonl").display().to_string());
    click(&app, &mut vcx, "statistics-source-save");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        assert!(state.management.error.is_some());
        assert_eq!(state.results[&(HostId::LOCAL, Some("omp"))].totals.tokens().total(), Some(3_000_000));
    });
    click(&app, &mut vcx, "statistics-editor-cancel");
    click(&app, &mut vcx, &format!("statistics-edit-source:{}", binding.key()));
    click(&app, &mut vcx, "statistics-source-save");
    settle(&app, &mut vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).active_settings().unwrap().statistics;
        assert_eq!(state.results[&(HostId::LOCAL, Some("omp"))].totals.tokens().total(), Some(3_000_000));
        assert!(state.results[&(HostId::LOCAL, Some("omp"))].bindings.iter().any(|b| b.key() == binding.key()));
    });
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
    for (locale, width, mode) in [
        ("en", 1120., gpui_kit::component::ThemeMode::Light),
        ("zh-CN", 620., gpui_kit::component::ThemeMode::Dark),
    ] {
        crate::ui::i18n::set_locale(locale);
        vcx.simulate_window_resize(window.into(), gpui_kit::size(px(width), px(800.)));
        app.update_in(&mut vcx, |_, window, cx| {
            gpui_kit::component::Theme::change(mode, Some(window), cx);
            gpui_kit::component::Theme::update(cx, |t| t.font_size = px(18.));
        });
        click(&app, &mut vcx, "statistics-manage-prices");
        click(&app, &mut vcx, "statistics-add-price");
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("statistics-price-save").is_none());
            assert!(app.read(cx).active_settings().unwrap().section == SettingsSection::Statistics);
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
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
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
}
