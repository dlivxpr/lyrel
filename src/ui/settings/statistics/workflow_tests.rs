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
        let scroll = &app.read(cx).active_settings().unwrap().content_scroll;
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
        window.press("secondary-a", cx);
        window.input(value, cx);
    });
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
    click(&app, &mut vcx, "statistics-add-source");
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
        click(&app, &mut vcx, "statistics-add-price");
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("statistics-price-save").is_none());
            assert!(app.read(cx).active_settings().unwrap().section == SettingsSection::Statistics);
        });
    }
    crate::ui::i18n::set_locale("en");
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
