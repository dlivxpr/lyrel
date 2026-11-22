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

fn store_scope(root: &str) -> lyrel_core::host::session_operations::SessionScope {
    let mut scope = lyrel_core::host::session_operations::SessionScope::default();
    scope.session_dir = Some(PathBuf::from(root));
    scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), root.into());
    scope
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
        '/',
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
        '/',
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
            '/',
            search,
            (None, None),
            now,
        )
        .into_iter()
        .map(|s| s.id)
        .collect::<Vec<_>>()
    };
    assert_eq!(
        pick(Some(CLIAgent::Pi), None, "", SessionActivity::Any, NOW),
        ["a", "c"]
    );
    assert_eq!(
        pick(None, Some(Path::new("/w")), "", SessionActivity::Any, NOW),
        ["a", "c"]
    );
    assert_eq!(
        pick(None, None, "B", SessionActivity::Any, NOW),
        ["b"],
        "search hits title"
    );
    assert_eq!(
        pick(None, None, "c", SessionActivity::Any, NOW),
        ["c"],
        "search hits the native id"
    );
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
        pick(
            None,
            Some(Path::new("/w")),
            "",
            SessionActivity::OlderThanThirtyDays,
            NOW
        ),
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
        '/',
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
        '/',
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
        '/',
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
    let mut open_scope = lyrel_core::host::session_operations::SessionScope::default();
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
    let listed = SessionIdentity::of(
        host,
        &PastSession {
            scope: Some({
                let mut scope = open_scope;
                // 扫描保留下它读到的文件；窗格 readout 没有 —— 不能因此漏判运行中。
                scope.session_file = Some(PathBuf::from("/store/pi/running.jsonl"));
                scope
            }),
            ..session(CLIAgent::Pi, "running", "/w", NOW)
        },
    );
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
        assert!(
            state.sessions.detail.is_some(),
            "detail survives a tab switch"
        );

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_agent(Some(CLIAgent::Pi));
        assert!(state.sessions.selected.is_empty(), "agent filter clears");

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_activity(SessionActivity::LastSevenDays);
        assert!(state.sessions.selected.is_empty(), "activity filter clears");

        state.sessions.set_row_selected("pi:a".into(), true);
        state.sessions.set_directory(Some(PathBuf::from("/w")));
        assert!(
            state.sessions.selected.is_empty(),
            "directory filter clears"
        );

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
    // 两条同名 ID 的行：同一主机、两个不同的存储目录。
    let mut listed_a = session(CLIAgent::Pi, "a", "/w", NOW);
    listed_a.scope = Some(store_scope("/store/a"));
    let mut listed_b = session(CLIAgent::Pi, "a", "/w", NOW);
    listed_b.scope = Some(store_scope("/store/b"));
    let identity_a = SessionIdentity::of(HostId::LOCAL, &listed_a);
    let identity_b = SessionIdentity::of(HostId::LOCAL, &listed_b);
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings_mut().expect("settings open");
        state.sessions.listing = listing(vec![listed_a.clone(), listed_b.clone()]);
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, cx| {
        let key_a = identity_a.key();
        settings.open_session_detail(key_a.clone(), cx);
        let state = settings.active_settings().unwrap();
        assert_eq!(state.sessions.detail.as_deref(), Some(key_a.as_str()));

        settings.set_sessions_hidden(true, vec![identity_a.clone()], cx);
        let hidden = &cx.global::<Config>().hidden_agent_sessions;
        assert!(
            hidden.contains(&identity_a.key()),
            "hide writes the scoped key: {}",
            identity_a.key()
        );
        assert!(
            !hidden.contains(&identity_a.legacy_key()),
            "the legacy spelling is never written"
        );
        assert!(
            !is_session_hidden(cx.global::<Config>(), &identity_b),
            "the same-id row in the other store stays visible"
        );
        let state = settings.active_settings().unwrap();
        assert!(
            state.sessions.selected.is_empty(),
            "hiding clears the batch selection"
        );
        assert_eq!(
            state.sessions.detail.as_deref(),
            Some(key_a.as_str()),
            "the resident detail keeps showing the now-hidden session"
        );

        settings.set_sessions_hidden(false, vec![identity_a.clone()], cx);
        assert!(
            !cx.global::<Config>()
                .hidden_agent_sessions
                .contains(&identity_a.key()),
            "unhide is reversible and Lyrel-only"
        );
    });
}

#[gpui_kit::test]
fn legacy_hide_redisplays_one_row_without_exposing_same_id_siblings(cx: &mut TestAppContext) {
    let (settings, mut vcx) = harness(cx);
    lyrel_ui_base::config::pin_test_config_dir();
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    // 旧版偏好：只有 slug:id。两个不同存储目录的同名 ID 行都因此隐藏。
    let mut listed_a = session(CLIAgent::Pi, "a", "/w", NOW);
    listed_a.scope = Some(store_scope("/store/a"));
    let mut listed_b = session(CLIAgent::Pi, "a", "/w", NOW);
    listed_b.scope = Some(store_scope("/store/b"));
    let identity_a = SessionIdentity::of(HostId::LOCAL, &listed_a);
    let identity_b = SessionIdentity::of(HostId::LOCAL, &listed_b);
    settings.update_in(&mut vcx, |settings, _, cx| {
        cx.update_global::<Config, _>(|cfg, _| {
            cfg.hidden_agent_sessions.insert(identity_a.legacy_key());
        });
        let state = settings.active_settings_mut().expect("settings open");
        state.sessions.listing = listing(vec![listed_a, listed_b]);
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, cx| {
        let state = settings.active_settings().unwrap();
        assert!(
            state
                .sessions
                .listing
                .sessions
                .iter()
                .all(|s| is_session_hidden(
                    cx.global::<Config>(),
                    &SessionIdentity::of(HostId::LOCAL, s)
                )),
            "both same-id rows read hidden through the one legacy entry"
        );

        settings.set_sessions_hidden(false, vec![identity_a.clone()], cx);
        let hidden = &cx.global::<Config>().hidden_agent_sessions;
        assert!(
            hidden.contains(&identity_a.legacy_key()),
            "the legacy entry survives, so the sibling stays hidden"
        );
        assert!(hidden.contains(&identity_a.show_key()));
        let state = settings.active_settings().unwrap();
        let visible = filtered_sessions(
            &state.sessions.listing.sessions,
            HostId::LOCAL,
            cx.global::<Config>(),
            SessionsTab::Visible,
            None,
            SessionActivity::Any,
            None,
            '/',
            "",
            (None, None),
            NOW,
        );
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible[0].cwd.as_deref(),
            Some(Path::new("/w")),
            "only the re-shown row came back"
        );
        assert_eq!(
            SessionIdentity::of(HostId::LOCAL, &visible[0]).key(),
            identity_a.key()
        );
        assert!(
            is_session_hidden(cx.global::<Config>(), &identity_b),
            "the same-id sibling stays hidden"
        );
    });
}

/// The directory filter judges the selected Host's records by that Host's
/// separator semantics: under a Linux host, `/w/Repo` and `/w/repo` are two
/// different directories even from a Windows client build.
#[test]
fn directory_filter_obeys_the_selected_host_separator() {
    let mut repo = session(CLIAgent::Pi, "repo", "/w/repo", NOW);
    repo.scope = Some(store_scope("/store/a"));
    let mut variant = session(CLIAgent::Pi, "variant", "/w/Repo", NOW);
    variant.scope = Some(store_scope("/store/a"));
    let cfg = Config::default();
    let rows = filtered_sessions(
        &[repo, variant],
        HostId::LOCAL,
        &cfg,
        SessionsTab::Visible,
        None,
        SessionActivity::Any,
        Some(Path::new("/w/repo")),
        '/',
        "",
        (None, None),
        NOW,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "repo");
    let windows_rows = filtered_sessions(
        &[session(CLIAgent::Pi, "repo", "C:\\w\\repo", NOW)],
        HostId::LOCAL,
        &cfg,
        SessionsTab::Visible,
        None,
        SessionActivity::Any,
        Some(Path::new("C:\\w\\REPO")),
        '\\',
        "",
        (None, None),
        NOW,
    );
    assert_eq!(
        windows_rows.len(),
        1,
        "a Windows host matches case-insensitively"
    );
}

/// 存储来源一行话给出确认到的实际位置：精确文件的所在路径，或没有文
/// 件时的提供方选择子（显式 Hermes home）；什么都确认不到才回落到默认
/// profile 文案。
#[test]
fn storage_label_shows_the_confirmed_actual_location() {
    let mut file_scope = store_scope("/store/pi");
    file_scope.session_file = Some(PathBuf::from("/store/pi/r/row.jsonl"));
    let with_file = SessionIdentity::of(
        HostId::LOCAL,
        &PastSession {
            scope: Some(file_scope),
            ..session(CLIAgent::Pi, "row", "/w", NOW)
        },
    );
    assert_eq!(
        storage_label(&with_file, None),
        "/store/pi/r/row.jsonl",
        "a located file's label is its actual location"
    );

    let mut hermes_scope = lyrel_core::host::session_operations::SessionScope::default();
    hermes_scope
        .environment
        .insert("HERMES_HOME".into(), "/srv/hermes".into());
    let hermes = SessionIdentity::of(
        HostId::LOCAL,
        &PastSession {
            scope: Some(hermes_scope),
            ..session(CLIAgent::Hermes, "row", "/w", NOW)
        },
    );
    assert_eq!(
        storage_label(&hermes, None),
        "/srv/hermes",
        "an explicit Hermes home is the store label"
    );

    let bare = SessionIdentity::of(HostId::LOCAL, &session(CLIAgent::Pi, "row", "/w", NOW));
    assert_eq!(
        storage_label(&bare, None),
        t(L10nKey::SessionsDefaultProfile),
        "nothing confirmed falls back to the default-profile wording"
    );
}
