//! 设置 → 会话 的测试：筛选、三态全选、不可删除资格等纯函数直接测；
//! 主机默认、页签/筛选清空勾选、隐藏往返走生产 Settings 消费者夹具。

use super::*;
use crate::ui::settings::test_window::harness;
use gpui_kit::{TestAppContext, px, size};
use lyrel_ui_base::config::Config;

const NOW: u64 = 1_760_000_000; // 2025-10-09，足够新的固定“现在”。

fn session(agent: CLIAgent, id: &str, cwd: &str, updated: u64) -> PastSession {
    PastSession {
        agent,
        id: id.into(),
        cwd: Some(PathBuf::from(cwd)),
        title: format!("title-{id}"),
        branch: None,
        updated,
        scope: None,
    }
}

fn listing(sessions: Vec<PastSession>) -> SessionListing {
    SessionListing {
        sessions,
        errors: Vec::new(),
        host_reachable: true,
    }
}

fn cfg_hiding(agent: CLIAgent, id: &str) -> Config {
    let mut cfg = Config::default();
    cfg.hidden_agent_sessions
        .insert(crate::core::agent_history::session_key(agent, id));
    cfg
}

#[test]
fn tabs_split_by_visibility_model_key() {
    let found = vec![
        session(CLIAgent::Pi, "a", "/w", NOW),
        session(CLIAgent::OhMyPi, "b", "/w", NOW),
    ];
    let cfg = cfg_hiding(CLIAgent::Pi, "a");
    let visible = filtered_sessions(
        &found,
        HostId::LOCAL,
        &cfg,
        SessionsTab::Visible,
        None,
        SessionActivity::Any,
        None,
        "",
        (None, None),
        NOW,
    );
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "b");
    let hidden = filtered_sessions(
        &found,
        HostId::LOCAL,
        &cfg,
        SessionsTab::Hidden,
        None,
        SessionActivity::Any,
        None,
        "",
        (None, None),
        NOW,
    );
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].id, "a");
}

#[test]
fn agent_directory_search_and_activity_filters_compose() {
    let found = vec![
        session(CLIAgent::Pi, "a", "/w", NOW - 3 * DAY_SECS),
        session(CLIAgent::OhMyPi, "b", "/other", NOW - 40 * DAY_SECS),
        session(CLIAgent::Pi, "c", "/w", NOW - 90 * DAY_SECS),
    ];
    let cfg = Config::default();
    let pick = |agent, dir, search, activity, now| {
        filtered_sessions(
            &found,
            HostId::LOCAL,
            &cfg,
            SessionsTab::Visible,
            agent,
            activity,
            dir,
            search,
            (None, None),
            now,
        )
        .into_iter()
        .map(|s| s.id)
        .collect::<Vec<_>>()
    };
    assert_eq!(pick(Some(CLIAgent::Pi), None, "", SessionActivity::Any, NOW), ["a", "c"]);
    assert_eq!(pick(None, Some(Path::new("/w")), "", SessionActivity::Any, NOW), ["a", "c"]);
    assert_eq!(pick(None, None, "B", SessionActivity::Any, NOW), ["b"], "search hits title");
    assert_eq!(pick(None, None, "c", SessionActivity::Any, NOW), ["c"], "search hits the native id");
    assert_eq!(
        pick(None, None, "", SessionActivity::LastSevenDays, NOW),
        ["a"]
    );
    assert_eq!(
        pick(None, None, "", SessionActivity::LastThirtyDays, NOW),
        ["a"]
    );
    assert_eq!(
        pick(None, None, "", SessionActivity::OlderThanThirtyDays, NOW),
        ["b", "c"]
    );
    // 目录 + 时间组合：/w 里早于 30 天的只有 c。
    assert_eq!(
        pick(None, Some(Path::new("/w")), "", SessionActivity::OlderThanThirtyDays, NOW),
        ["c"]
    );
}

#[test]
fn unknown_activity_never_reads_as_older_than_thirty_days() {
    let found = vec![session(CLIAgent::Pi, "a", "/w", 0)];
    let cfg = Config::default();
    let rows = filtered_sessions(
        &found,
        HostId::LOCAL,
        &cfg,
        SessionsTab::Visible,
        None,
        SessionActivity::OlderThanThirtyDays,
        None,
        "",
        (None, None),
        NOW,
    );
    assert!(rows.is_empty(), "updated == 0 is unknown, not old");
}

#[test]
fn custom_range_is_inclusive_on_both_ends() {
    let found = vec![
        session(CLIAgent::Pi, "edge-start", "/w", NOW - 2 * DAY_SECS),
        session(CLIAgent::Pi, "middle", "/w", NOW - DAY_SECS),
        session(CLIAgent::Pi, "edge-end", "/w", NOW),
        session(CLIAgent::Pi, "outside", "/w", NOW + DAY_SECS),
    ];
    let cfg = Config::default();
    let day = |offset: i64| {
        chrono::DateTime::from_timestamp((NOW as i64) + offset * DAY_SECS as i64, 0)
            .unwrap()
            .date_naive()
    };
    let rows = filtered_sessions(
        &found,
        HostId::LOCAL,
        &cfg,
        SessionsTab::Visible,
        None,
        SessionActivity::Custom,
        None,
        "",
        (Some(day(-2)), Some(day(0))),
        NOW,
    );
    let ids: Vec<_> = rows.into_iter().map(|s| s.id).collect();
    assert_eq!(ids, ["edge-start", "middle", "edge-end"]);
    // 一侧解析失败按不限处理：只有起始日，终侧放行。
    let open = filtered_sessions(
        &found,
        HostId::LOCAL,
        &cfg,
        SessionsTab::Visible,
        None,
        SessionActivity::Custom,
        None,
        "",
        (Some(day(-2)), None),
        NOW,
    );
    assert_eq!(open.len(), 4);
}

#[test]
fn select_all_state_tracks_checked_rows() {
    let rows = vec![
        SessionRow {
            session: session(CLIAgent::Pi, "a", "/w", NOW),
            key: "pi:a".into(),
            blocked: Vec::new(),
        },
        SessionRow {
            session: session(CLIAgent::Pi, "b", "/w", NOW),
            key: "pi:b".into(),
            blocked: Vec::new(),
        },
    ];
    let mut selected = BTreeSet::new();
    assert_eq!(select_all_state(&selected, &rows), SelectAllState::None);
    selected.insert("pi:a".into());
    assert_eq!(select_all_state(&selected, &rows), SelectAllState::Some);
    selected.insert("pi:b".into());
    assert_eq!(select_all_state(&selected, &rows), SelectAllState::All);
    // 勾选可以来自其它筛选结果 —— 当前结果全未勾选时仍是 None。
    selected.insert("pi:elsewhere".into());
    let rows_now: Vec<SessionRow> = Vec::new();
    assert_eq!(select_all_state(&selected, &rows_now), SelectAllState::None);
}

#[test]
fn blockers_name_running_unsupported_and_unconfirmed() {
    let host = HostId::LOCAL;
    let mut open_scope =
        lyrel_core::host::session_operations::SessionScope::default();
    open_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store/pi".into());
    let open = SessionIdentity {
        host,
        agent: CLIAgent::Pi,
        id: "running".into(),
        cwd: Some(PathBuf::from("/w")),
        scope: Some(open_scope.clone()),
    };
    let listed = SessionIdentity::of(host, &PastSession {
        scope: Some({
            let mut scope = open_scope;
            // 扫描保留下它读到的文件；窗格 readout 没有 —— 不能因此漏判运行中。
            scope.session_file = Some(PathBuf::from("/store/pi/running.jsonl"));
            scope
        }),
        ..session(CLIAgent::Pi, "running", "/w", NOW)
    });
    assert_eq!(
        session_blockers(&listed, &[open]),
        vec![SessionBlocker::Running]
    );

    let opencode = SessionIdentity::of(
        host,
        &PastSession {
            scope: Some(lyrel_core::host::session_operations::SessionScope {
                environment: std::collections::BTreeMap::from([(
                    "OPENCODE_DB".into(),
                    "/store/oc.db".into(),
                )]),
                ..Default::default()
            }),
            ..session(CLIAgent::OpenCode, "svc", "/w", NOW)
        },
    );
    assert_eq!(
        session_blockers(&opencode, &[]),
        vec![SessionBlocker::SourceUnsupported],
        "a confirmed OpenCode store still has no deletion path"
    );

    let bare = SessionIdentity::of(host, &session(CLIAgent::Hermes, "bare", "/w", NOW));
    assert_eq!(
        session_blockers(&bare, &[]),
        vec![SessionBlocker::StoreUnconfirmed]
    );
}

#[gpui_kit::test]
fn sessions_page_defaults_to_workspace_host_and_lays_out(cx: &mut TestAppContext) {
    let (settings, mut vcx) = harness(cx);
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, _cx| {
        let state = settings.active_settings().expect("settings open");
        assert!(state.navigation.section() == SettingsSection::Sessions);
        assert_eq!(
            state.sessions.host,
            HostId::LOCAL,
            "one host at a time, defaulting to the workspace that opened settings"
        );
    });
    // 种子列表 + 最窄窗口布局：渲染路径（工具栏、页签、全选、详情栏）不 panic。
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings_mut().expect("settings open");
        state.sessions.listing = listing(vec![
            session(CLIAgent::Pi, "a", "/w", NOW),
            session(CLIAgent::OhMyPi, "b", "/w", NOW - DAY_SECS),
        ]);
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
    });
    vcx.simulate_resize(size(px(720.), px(560.)));
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn host_tab_and_filter_moves_clear_selection(cx: &mut TestAppContext) {
    let (settings, mut vcx) = harness(cx);
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
        let state = settings.active_settings_mut().unwrap();
        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_detail(Some("pi:a".into()));
        state.sessions.set_tab(SessionsTab::Hidden);
        assert!(state.sessions.selected.is_empty(), "tab switch clears");
        assert!(state.sessions.detail.is_some(), "detail survives a tab switch");

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_agent(Some(CLIAgent::Pi));
        assert!(state.sessions.selected.is_empty(), "agent filter clears");

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_activity(SessionActivity::LastSevenDays);
        assert!(state.sessions.selected.is_empty(), "activity filter clears");

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_directory(Some(PathBuf::from("/w")));
        assert!(state.sessions.selected.is_empty(), "directory filter clears");

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.select_host(HostId(u64::MAX));
        assert!(state.sessions.selected.is_empty(), "host switch clears");
        assert!(
            state.sessions.detail.is_none(),
            "the resident detail belongs to the previous host's listing"
        );
        assert_eq!(state.sessions.host, HostId(u64::MAX));
    });
}

#[gpui_kit::test]
fn hide_and_unhide_round_trip_through_the_visibility_key(cx: &mut TestAppContext) {
    let (settings, mut vcx) = harness(cx);
    lyrel_ui_base::config::pin_test_config_dir();
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.open_session_detail("pi:a".into(), cx);
        let state = settings.active_settings().unwrap();
        assert_eq!(state.sessions.detail.as_deref(), Some("pi:a"));

        settings.set_sessions_hidden(true, vec!["pi:a".into()], cx);
        assert!(
            cx.global::<Config>()
                .hidden_agent_sessions
                .contains("pi:a"),
            "hide writes the same `slug:id` key the other consumers read"
        );
        let state = settings.active_settings().unwrap();
        assert!(
            state.sessions.selected.is_empty(),
            "hiding clears the batch selection"
        );
        assert_eq!(
            state.sessions.detail.as_deref(),
            Some("pi:a"),
            "the resident detail keeps showing the now-hidden session"
        );

        settings.set_sessions_hidden(false, vec!["pi:a".into()], cx);
        assert!(
            !cx.global::<Config>()
                .hidden_agent_sessions
                .contains("pi:a"),
            "unhide is reversible and Lyrel-only"
        );
    });
}
