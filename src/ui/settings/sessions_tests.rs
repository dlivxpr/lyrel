//! 设置 → 会话 的测试：筛选、三态全选、不可删除资格等纯函数直接测；
//! 主机默认、页签/筛选清空勾选、隐藏往返走生产 Settings 消费者夹具。

use super::*;
use crate::ui::settings::test_window::{harness, harness_standalone};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
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
        any_listed: true,
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
            .with_timezone(&chrono::Local)
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
fn custom_range_follows_the_displayed_local_day() {
    use chrono::TimeZone as _;

    let timezone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let local_midnight = timezone.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
    let day = local_midnight.date_naive();
    let range = (Some(day), Some(day));
    let start = local_midnight.timestamp() as u64;
    assert!(!matches_calendar_dates(start - 1, range, &timezone));
    assert!(matches_calendar_dates(start, range, &timezone));
    assert!(matches_calendar_dates(
        start + DAY_SECS - 1,
        range,
        &timezone
    ));
    assert!(!matches_calendar_dates(start + DAY_SECS, range, &timezone));
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
        vec![SessionBlocker::Running, SessionBlocker::PiNoWriteLease],
        "a live pane blocks first; the provider's own gap stays visible beside it"
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
        vec![SessionBlocker::OpenCodeCascadeUnfenced],
        "OpenCode deletes interrupt active sessions and cascade without a fenceable set"
    );

    let bare = SessionIdentity::of(host, &session(CLIAgent::Hermes, "bare", "/w", NOW));
    assert_eq!(
        session_blockers(&bare, &[]),
        vec![SessionBlocker::StoreUnconfirmed]
    );
}

#[gpui_kit::test]
fn sessions_page_starts_on_local_host_with_all_directories_and_lays_out(cx: &mut TestAppContext) {
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
            "the page starts on this computer no matter which workspace opened settings"
        );
        assert!(
            state.sessions.directory.is_none(),
            "the page starts on All Directories"
        );
    });
    // Populate the real page before measuring the narrow layout.
    settings.update_in(&mut vcx, |settings, _, cx| {
        let state = settings.active_settings_mut().expect("settings open");
        state.sessions.listing = listing(vec![
            session(CLIAgent::Pi, "a", "/w", NOW),
            session(CLIAgent::OhMyPi, "b", "/w", NOW - DAY_SECS),
        ]);
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
        state.sessions.connection = HostRegistry::lookup(cx, HostId::LOCAL);
    });
    vcx.simulate_resize(size(px(720.), px(560.)));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let header = window.find("sessions-host-trigger").bounds();
        let body = window.find("settings-sessions").bounds();
        let toolbar = window.find("sessions-toolbar").bounds();
        let side = window.find("sessions-side").bounds();
        assert!(toolbar.top() >= header.bottom());
        assert!(toolbar.top() - header.bottom() <= window.rem_size() * 2.);
        assert!(side.top() >= toolbar.bottom());
        assert_eq!(side.bottom(), body.bottom());
        for id in [
            "sessions-agent-trigger",
            "sessions-activity-trigger",
            "sessions-directory-trigger",
        ] {
            let filter = window.find(id).bounds();
            assert!(filter.right() <= toolbar.right(), "{id} is clipped");
            assert!(
                filter.bottom() <= toolbar.bottom(),
                "{id} overlaps the list"
            );
        }
    });
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
        let prior = listing(vec![session(CLIAgent::Pi, "old-host", "/w", NOW)]);
        state.sessions.listing = prior.clone();
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
        let prior_generation = state.sessions.generation;
        let prior_session = state.navigation.session();
        state.sessions.select_host(HostId(u64::MAX));
        assert!(state.sessions.selected.is_empty(), "host switch clears");
        assert!(
            state.sessions.detail.is_none(),
            "the resident detail belongs to the previous host's listing"
        );
        assert_eq!(state.sessions.host, HostId(u64::MAX));
        assert!(state.sessions.directory.is_none());
        settings.sessions_settings_landed(
            prior_session,
            prior_generation,
            HostId::LOCAL,
            prior,
            cx,
        );
        assert!(settings.sessions_source().0.is_empty());
    });
}

// ---- 目录筛选：菜单固定两项、本机系统选择器、SSH 只读浏览器 ----

/// The owning Host behind the directory-browser tests: POSIX spellings,
/// canned listings per directory, and a record of every directory it was
/// asked to list — the flow runs through the real HostOps wiring without
/// touching a native store.
struct DirectoryHost {
    inner: lyrel_core::host::SharedHost,
    listed: std::sync::Mutex<Vec<PathBuf>>,
}

impl DirectoryHost {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner: lyrel_core::host::local::shared(),
            listed: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn entry(name: &str, is_dir: bool, is_symlink: bool) -> lyrel_core::host::Entry {
        lyrel_core::host::Entry {
            name: name.into(),
            is_dir,
            is_symlink,
            ignored: false,
        }
    }

    fn listed(&self) -> Vec<PathBuf> {
        self.listed.lock().unwrap().clone()
    }
}

impl lyrel_core::host::Host for DirectoryHost {
    fn id(&self) -> HostId {
        HostId(0x5ca1_ab1e)
    }
    fn separator(&self) -> char {
        '/'
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(
        &self,
        dir: &Path,
        _root: Option<&Path>,
    ) -> std::io::Result<Vec<lyrel_core::host::Entry>> {
        self.listed.lock().unwrap().push(dir.to_path_buf());
        let names: &[&str] = match dir.to_string_lossy().as_ref() {
            "/home/dev" => &["projects", "notes.txt", "work", "link"],
            "/home" => &["dev", "ops"],
            "/home/dev/projects" => &["lyrel"],
            _ => &[],
        };
        Ok(names
            .iter()
            .map(|name| match *name {
                "notes.txt" => Self::entry(name, false, false),
                "link" => Self::entry(name, true, true),
                _ => Self::entry(name, true, false),
            })
            .collect())
    }
    fn stat(&self, p: &Path) -> std::io::Result<lyrel_core::host::Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        dirs: usize,
        hidden: bool,
    ) -> std::io::Result<Vec<lyrel_core::host::SearchHit>> {
        self.inner.search(roots, query, limit, dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> std::io::Result<lyrel_core::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> std::io::Result<lyrel_core::host::Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> std::io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> std::io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<lyrel_core::host::Output> {
        self.inner.git(cwd, args)
    }
    fn shells(&self) -> std::io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<lyrel_core::host::WatchSub> {
        self.inner.watch(dirs)
    }
}

fn settle_directory_browser(app: &Entity<SettingsWindow>, vcx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if app.update_in(vcx, |app, _, _| {
            app.active_settings()
                .and_then(|s| s.sessions.directory_browser.as_ref())
                .is_some_and(|browser| !browser.loading)
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "directory listing did not land"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn browser_names(app: &Entity<SettingsWindow>, vcx: &mut VisualTestContext) -> Vec<String> {
    app.update_in(vcx, |app, _, _| {
        app.active_settings()
            .and_then(|s| s.sessions.directory_browser.as_ref())
            .map(|browser| {
                browser
                    .entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn directory_menu_lists_all_directories_and_choose_only(cx: &mut TestAppContext) {
    let (settings, mut vcx) = harness(cx);
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, _| {
        settings
            .active_settings_mut()
            .unwrap()
            .sessions
            .set_directory(Some(PathBuf::from("/w")));
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("sessions-directory-trigger", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find("sessions-directory-item-0").visible(),
            "All Directories is the first entry"
        );
        assert!(
            window.find("sessions-directory-item-1").visible(),
            "Choose Directory is the second entry"
        );
        assert!(
            window.try_find("sessions-directory-item-2").is_none(),
            "the menu holds exactly the two entries — no discovered folders, no search"
        );
    });
    vcx.update(|window, cx| {
        window.click("sessions-directory-item-0", cx);
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        assert!(
            settings
                .active_settings()
                .unwrap()
                .sessions
                .directory
                .is_none(),
            "All Directories clears the directory filter"
        );
    });
}

#[gpui_kit::test]
fn ssh_directory_browser_browses_by_host_paths_and_confirms(cx: &mut TestAppContext) {
    let (settings, _, mut vcx) = harness_standalone(cx);
    let host = DirectoryHost::new();
    let shared: lyrel_core::host::SharedHost = host.clone();
    settings.update_in(&mut vcx, |_, _window, cx| {
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, shared.clone());
        lyrel_ui_base::desktop_lookups::publish_remote_home(
            cx,
            shared.id(),
            Some(PathBuf::from("/home/dev")),
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, _| {
        settings
            .active_settings_mut()
            .unwrap()
            .sessions
            .select_host(shared.id());
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.choose_sessions_directory(window, cx);
    });
    settle_directory_browser(&settings, &mut vcx);

    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings().unwrap();
        let browser = state.sessions.directory_browser.as_ref().unwrap();
        assert_eq!(
            browser.current,
            PathBuf::from("/home/dev"),
            "the browser starts on the host's published home"
        );
        assert_eq!(
            browser
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["projects", "work"],
            "files and symlinks stay out of a read-only directory browser"
        );
    });
    // 上级：按这台主机的分隔符语义退一级（不是本机 Path 语义）。
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("sessions-directory-up", cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    settings.update_in(&mut vcx, |settings, _, _| {
        let browser = settings
            .active_settings()
            .unwrap()
            .sessions
            .directory_browser
            .as_ref()
            .unwrap();
        assert_eq!(browser.current, PathBuf::from("/home"));
    });
    assert_eq!(
        browser_names(&settings, &mut vcx),
        ["dev", "ops"],
        "directories only, sorted by the host's own names"
    );
    // 子目录：用主机的 join 拼写进入，客户端不重新解释。
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("sessions-directory-entry-dev", cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    settings.update_in(&mut vcx, |settings, _, _| {
        let browser = settings
            .active_settings()
            .unwrap()
            .sessions
            .directory_browser
            .as_ref()
            .unwrap();
        assert_eq!(browser.current, PathBuf::from("/home/dev"));
    });
    assert_eq!(
        host.listed(),
        vec![
            PathBuf::from("/home/dev"),
            PathBuf::from("/home"),
            PathBuf::from("/home/dev"),
        ],
        "every listing so far ran against the owning host, in navigation order"
    );
    // Home 一键回到发布过的 home。
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("sessions-directory-home", cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    settings.update_in(&mut vcx, |settings, _, _| {
        let browser = settings
            .active_settings()
            .unwrap()
            .sessions
            .directory_browser
            .as_ref()
            .unwrap();
        assert_eq!(browser.current, PathBuf::from("/home/dev"));
    });
    // 选定：写进目录筛选并关掉浏览器。
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("sessions-directory-confirm", cx);
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings().unwrap();
        assert_eq!(
            state.sessions.directory.as_deref(),
            Some(Path::new("/home/dev"))
        );
        assert!(state.sessions.directory_browser.is_none());
    });
}

#[gpui_kit::test]
fn directory_browser_landing_rejects_stale_answers(cx: &mut TestAppContext) {
    let (settings, _, mut vcx) = harness_standalone(cx);
    let host = DirectoryHost::new();
    let shared: lyrel_core::host::SharedHost = host.clone();
    settings.update_in(&mut vcx, |_, _window, cx| {
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, shared.clone());
        lyrel_ui_base::desktop_lookups::publish_remote_home(
            cx,
            shared.id(),
            Some(PathBuf::from("/home/dev")),
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, _| {
        settings
            .active_settings_mut()
            .unwrap()
            .sessions
            .select_host(shared.id());
    });
    let session = settings.update_in(&mut vcx, |settings, _, _| {
        settings.active_settings().unwrap().navigation.session()
    });
    // 没有浏览器时，落地的答案无处可去。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            1,
            shared.id(),
            shared.clone(),
            Ok(vec![DirectoryHost::entry("orphan", true, false)]),
            cx,
        );
        assert!(
            settings
                .active_settings()
                .unwrap()
                .sessions
                .directory_browser
                .is_none()
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.choose_sessions_directory(window, cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    // 取消再开：旧浏览器作废，之后的旧答案不能借新旅程落地。
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("sessions-directory-cancel", cx);
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        assert!(
            settings
                .active_settings()
                .unwrap()
                .sessions
                .directory_browser
                .is_none(),
            "cancel drops the browser"
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.choose_sessions_directory(window, cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    let token = settings.update_in(&mut vcx, |settings, _, _| {
        settings
            .active_settings()
            .unwrap()
            .sessions
            .directory_browser
            .as_ref()
            .unwrap()
            .token
    });
    // 同一会话、同一主机、同一连接、同一令牌才落地。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            token,
            shared.id(),
            shared.clone(),
            Ok(vec![DirectoryHost::entry("fresh", true, false)]),
            cx,
        );
    });
    assert_eq!(browser_names(&settings, &mut vcx), ["fresh"]);
    // 退休的旧令牌（上一次打开/导航的数字）：丢弃。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            token - 1,
            shared.id(),
            shared.clone(),
            Ok(vec![DirectoryHost::entry("stale", true, false)]),
            cx,
        );
    });
    assert_eq!(
        browser_names(&settings, &mut vcx),
        ["fresh"],
        "an answer from a retired journey is dropped even after cancel/reopen"
    );
    // 别的主机：丢弃。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            token,
            HostId(u64::MAX),
            shared.clone(),
            Ok(vec![DirectoryHost::entry("stale", true, false)]),
            cx,
        );
    });
    assert_eq!(browser_names(&settings, &mut vcx), ["fresh"]);
    // 同一主机身份、不同的连接 Arc：丢弃。
    let reconnected: lyrel_core::host::SharedHost = DirectoryHost::new();
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            token,
            shared.id(),
            reconnected,
            Ok(vec![DirectoryHost::entry("stale", true, false)]),
            cx,
        );
    });
    assert_eq!(
        browser_names(&settings, &mut vcx),
        ["fresh"],
        "a landing from a superseded connection is dropped"
    );
    // 重连后旧浏览器不再替新连接背书：选定被丢弃，筛选不落定。
    let replacement: lyrel_core::host::SharedHost = DirectoryHost::new();
    settings.update_in(&mut vcx, |_, _window, cx| {
        lyrel_ui_base::host_registry::HostRegistry::remove(cx, shared.id());
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, replacement.clone());
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.confirm_sessions_directory(window, cx);
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings().unwrap();
        assert!(state.sessions.directory.is_none());
        assert!(
            state.sessions.directory_browser.is_none(),
            "confirm against a reconnected host drops the browser"
        );
    });
    // 列失败留下错误态；此时选定不放行，浏览器留着展示错误。
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.choose_sessions_directory(window, cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    let token = settings.update_in(&mut vcx, |settings, _, _| {
        settings
            .active_settings()
            .unwrap()
            .sessions
            .directory_browser
            .as_ref()
            .unwrap()
            .token
    });
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            token,
            replacement.id(),
            replacement.clone(),
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
            cx,
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.confirm_sessions_directory(window, cx);
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings().unwrap();
        assert!(
            state.sessions.directory.is_none(),
            "a failed listing cannot be confirmed"
        );
        assert!(
            state.sessions.directory_browser.is_some(),
            "the browser stays open so the error is visible"
        );
    });
    // 同一路径重新列成功即可选定。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.sessions_directory_landed(
            session,
            token,
            replacement.id(),
            replacement.clone(),
            Ok(vec![DirectoryHost::entry("fresh", true, false)]),
            cx,
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.confirm_sessions_directory(window, cx);
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings().unwrap();
        assert_eq!(
            state.sessions.directory.as_deref(),
            Some(Path::new("/home/dev"))
        );
        assert!(state.sessions.directory_browser.is_none());
    });
}

#[gpui_kit::test]
fn switching_host_cancels_the_directory_browser(cx: &mut TestAppContext) {
    let (settings, _, mut vcx) = harness_standalone(cx);
    let host = DirectoryHost::new();
    let shared: lyrel_core::host::SharedHost = host.clone();
    settings.update_in(&mut vcx, |_, _window, cx| {
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, shared.clone());
        lyrel_ui_base::desktop_lookups::publish_remote_home(
            cx,
            shared.id(),
            Some(PathBuf::from("/home/dev")),
        );
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |settings, _, _| {
        settings
            .active_settings_mut()
            .unwrap()
            .sessions
            .select_host(shared.id());
    });
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.choose_sessions_directory(window, cx);
    });
    settle_directory_browser(&settings, &mut vcx);
    settings.update_in(&mut vcx, |settings, _, _| {
        assert!(
            settings
                .active_settings()
                .unwrap()
                .sessions
                .directory_browser
                .is_some()
        );
    });
    settings.update_in(&mut vcx, |settings, _, _| {
        let state = settings.active_settings_mut().unwrap();
        state
            .sessions
            .set_directory(Some(PathBuf::from("/home/dev/projects")));
        state.sessions.select_host(HostId::LOCAL);
        assert!(
            state.sessions.directory.is_none(),
            "switching host resets the directory filter"
        );
        assert!(
            state.sessions.directory_browser.is_none(),
            "switching host cancels the directory browser"
        );
    });
}

#[gpui_kit::test]
fn hide_and_unhide_round_trip_through_the_visibility_key(cx: &mut TestAppContext) {
    let (settings, _, mut vcx) = harness_standalone(cx);
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
    settings.update_in(&mut vcx, |settings, _, cx| {
        let state = settings.active_settings_mut().expect("settings open");
        state.sessions.listing = listing(vec![listed_a.clone(), listed_b.clone()]);
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
        state.sessions.connection = HostRegistry::lookup(cx, HostId::LOCAL);
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
    let (settings, _, mut vcx) = harness_standalone(cx);
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
        state.sessions.connection = HostRegistry::lookup(cx, HostId::LOCAL);
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

// ---- 批量原生删除（#367）：分组、资格门槛与 prepare/execute 全流程 ----

use lyrel_core::host::session_operations::batch::{
    SessionBatchExecuteResult, SessionBatchGroupResult, SessionBatchItem,
    SessionBatchPrepareResult, SessionBatchReason, SessionBatchReasonKind, SessionBatchReply,
    SessionBatchRequest, SessionBatchStatus, SessionBatchTotals,
};

fn hermes_session(id: &str, home: &str, cwd: &str) -> PastSession {
    let mut scope = lyrel_core::host::session_operations::SessionScope::default();
    if !home.is_empty() {
        scope.environment.insert("HERMES_HOME".into(), home.into());
    }
    PastSession {
        agent: CLIAgent::Hermes,
        id: id.into(),
        cwd: Some(PathBuf::from(cwd)),
        title: format!("title-{id}"),
        branch: None,
        updated: NOW,
        scope: Some(scope),
    }
}

fn candidate(session: &PastSession, live: &[SessionIdentity]) -> SessionDeleteCandidate {
    let identity = SessionIdentity::of(HostId::LOCAL, session);
    SessionDeleteCandidate {
        blocked: session_blockers(&identity, live),
        title: session.title.clone().into(),
        identity,
        cwd: session.cwd.clone(),
    }
}

#[test]
fn delete_gate_only_hermes_with_confirmed_store() {
    let listed = hermes_session("h", "/store/hermes", "/w");
    let identity = SessionIdentity::of(HostId::LOCAL, &listed);
    assert!(
        session_blockers(&identity, &[]).is_empty(),
        "a Hermes row with a confirmed store reads deletable"
    );
    assert_eq!(
        session_blockers(&identity, &[identity.clone()]),
        [SessionBlocker::Running],
        "a live pane at the same identity blocks first"
    );

    let kimi = session(CLIAgent::Kimi, "k", "/w", NOW);
    let kimi_identity = SessionIdentity::of(HostId::LOCAL, &kimi);
    assert_eq!(
        session_blockers(&kimi_identity, &[]),
        [SessionBlocker::KimiDeleteUnfenced],
        "Kimi ACP delete can fail open without an expected identity fence"
    );

    let bare = hermes_session("bare", "", "/w");
    let bare_identity = SessionIdentity::of(HostId::LOCAL, &bare);
    assert_eq!(
        session_blockers(&bare_identity, &[]),
        [SessionBlocker::StoreUnconfirmed],
        "a Hermes row whose store cannot be named is not deletable"
    );
}

#[test]
fn delete_plan_groups_by_store_and_keeps_selection_order() {
    let candidates = vec![
        candidate(&hermes_session("a", "/store/one", "/w"), &[]),
        candidate(&hermes_session("b", "/store/two", "/w"), &[]),
        candidate(&hermes_session("c", "/store/one", "/w2"), &[]),
        SessionDeleteCandidate {
            blocked: vec![SessionBlocker::Running],
            ..candidate(&hermes_session("run", "/store/one", "/w"), &[])
        },
    ];
    let (rows, groups) = build_delete_plan(&candidates, None, None, &Default::default());
    assert_eq!(groups.len(), 2, "two distinct stores, two groups");
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows[0].group,
        Some(0),
        "first store claimed the first group in selection order"
    );
    assert_eq!(rows[1].group, Some(1));
    assert_eq!(
        rows[2].group,
        Some(0),
        "the same store reuses its group however the selection interleaves"
    );
    assert_eq!(rows[3].group, None, "a blocked row joins no group");
    assert_eq!(
        rows[3].excluded,
        Some(SessionBlocker::Running),
        "the blocked row keeps its actionable reason"
    );
    assert_eq!(
        groups[0]
            .scope
            .environment
            .get("HERMES_HOME")
            .map(String::as_str),
        Some("/store/one")
    );
}

/// The owning Host behind the batch flow: records what was asked and
/// answers what the test queued, so the whole settings flow runs through
/// the real prepare/execute wiring without touching a native store.
struct BatchHost {
    inner: lyrel_core::host::SharedHost,
    seen: std::sync::Mutex<Vec<SessionBatchRequest>>,
    queue: std::sync::Mutex<Vec<SessionBatchReply>>,
}

impl BatchHost {
    fn new(inner: lyrel_core::host::SharedHost) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner,
            seen: std::sync::Mutex::new(Vec::new()),
            queue: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn push(&self, reply: SessionBatchReply) {
        self.queue.lock().unwrap().push(reply);
    }

    fn seen(&self) -> Vec<SessionBatchRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl lyrel_core::host::Host for BatchHost {
    fn id(&self) -> HostId {
        self.inner.id()
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(
        &self,
        dir: &Path,
        root: Option<&Path>,
    ) -> std::io::Result<Vec<lyrel_core::host::Entry>> {
        self.inner.read_dir(dir, root)
    }
    fn stat(&self, p: &Path) -> std::io::Result<lyrel_core::host::Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        dirs: usize,
        hidden: bool,
    ) -> std::io::Result<Vec<lyrel_core::host::SearchHit>> {
        self.inner.search(roots, query, limit, dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> std::io::Result<lyrel_core::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> std::io::Result<lyrel_core::host::Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> std::io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> std::io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<lyrel_core::host::Output> {
        self.inner.git(cwd, args)
    }
    fn shells(&self) -> std::io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<lyrel_core::host::WatchSub> {
        self.inner.watch(dirs)
    }
    fn session_batch(
        &self,
        request: &SessionBatchRequest,
    ) -> Result<SessionBatchReply, lyrel_core::host::session_operations::SessionOperationError>
    {
        self.seen.lock().unwrap().push(request.clone());
        let mut queue = self.queue.lock().unwrap();
        match queue.is_empty() {
            true => Err(
                lyrel_core::host::session_operations::SessionOperationError {
                    kind: lyrel_core::host::session_operations::SessionErrorKind::Unsupported,
                    message: "no canned answer".into(),
                    retained: None,
                },
            ),
            false => Ok(queue.remove(0)),
        }
    }
}

#[gpui_kit::test]
fn batch_delete_flow_prepare_confirm_execute_and_refresh(cx: &mut TestAppContext) {
    let (settings, mut vcx) = harness(cx);
    lyrel_ui_base::config::pin_test_config_dir();
    let host = BatchHost::new(lyrel_core::host::local::shared());
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.open_settings_section(SettingsSection::Sessions, window, cx);
    });
    vcx.run_until_parked();
    settings.update_in(&mut vcx, |_, _window, cx| {
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, host.clone());
    });

    let listed_a = hermes_session("a", "/store/one", "/w");
    let listed_b = hermes_session("b", "/store/two", "/w");
    let listed_c = hermes_session("c", "/store/one", "/w2");
    let keys: Vec<String> = [&listed_a, &listed_b, &listed_c]
        .iter()
        .map(|session| SessionIdentity::of(HostId::LOCAL, session).key())
        .collect();
    settings.update_in(&mut vcx, |settings, _, cx| {
        let state = settings.active_settings_mut().expect("settings open");
        state.sessions.listing =
            listing(vec![listed_a.clone(), listed_b.clone(), listed_c.clone()]);
        state.sessions.landed = true;
        state.sessions.bound = Some(HostId::LOCAL);
        state.sessions.connection = Some(host.clone());
        for key in &keys {
            state.sessions.selected.insert(key.clone());
        }
        cx.update_global::<Config, _>(|cfg, _| {
            cfg.agent_launch.insert("hermes".into(), "hermes".into());
        });
    });

    host.push(SessionBatchReply::Prepare(SessionBatchPrepareResult {
        groups: vec![
            SessionBatchGroupResult {
                items: vec![
                    SessionBatchItem::eligible("a".into(), vec!["a".into(), "a.child".into()]),
                    SessionBatchItem::eligible("c".into(), vec!["c".into()]),
                ],
            },
            SessionBatchGroupResult {
                items: vec![SessionBatchItem::eligible("b".into(), vec!["b".into()])],
            },
        ],
    }));
    let candidates = vec![
        candidate(&listed_a, &[]),
        candidate(&listed_b, &[]),
        candidate(&listed_c, &[]),
    ];
    settings.update_in(&mut vcx, |settings, window, cx| {
        settings.start_session_delete(window, candidates, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if settings.update_in(&mut vcx, |settings, _, _| {
            settings
                .active_settings()
                .and_then(|state| state.sessions.delete.as_ref())
                .is_some_and(|flow| !matches!(flow.phase, SessionDeletePhase::Preparing))
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "prepare did not land");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    settings.update_in(&mut vcx, |settings, _, _| {
        let flow = settings
            .active_settings()
            .unwrap()
            .sessions
            .delete
            .as_ref()
            .expect("flow opened");
        assert!(
            matches!(flow.phase, SessionDeletePhase::Ready),
            "the canned prepare answer landed"
        );
        assert_eq!(flow.eligible(), 3);
        assert_eq!(flow.groups.len(), 2);
        let plans: Vec<Option<&SessionDeletePlan>> =
            flow.rows.iter().map(|row| row.plan.as_ref()).collect();
        match plans[0] {
            Some(SessionDeletePlan::Eligible { delete_ids }) => assert_eq!(
                delete_ids.as_slice(),
                ["a", "a.child"],
                "the exact native cascade is bound to the row"
            ),
            _ => panic!("row a should be eligible"),
        }
        match plans[1] {
            Some(SessionDeletePlan::Eligible { delete_ids }) => assert_eq!(
                delete_ids.as_slice(),
                ["b"],
                "each row carries only its own cascade"
            ),
            _ => panic!("row b should be eligible"),
        }
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        for (element, expected) in [
            ("sessions-delete-target-0-0", "a"),
            ("sessions-delete-target-0-1", "a.child"),
            ("sessions-delete-target-1-0", "b"),
        ] {
            let observed = window.find(element);
            assert!(observed.visible(), "confirmation hides {expected}");
            assert_eq!(observed.label(), Some(expected));
        }
    });

    let (events, _subscription) = crate::ui::settings::test_window::record_events(&settings, cx);
    // 同意清单：两项都勾选才能确认。execute 的回复先入队，确认即消费。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.confirm_session_delete(cx);
        assert!(matches!(
            settings
                .active_settings()
                .unwrap()
                .sessions
                .delete
                .as_ref()
                .unwrap()
                .phase,
            SessionDeletePhase::Ready
        ));
        settings.set_session_delete_consent(0, true, cx);
        settings.confirm_session_delete(cx);
        assert!(matches!(
            settings
                .active_settings()
                .unwrap()
                .sessions
                .delete
                .as_ref()
                .unwrap()
                .phase,
            SessionDeletePhase::Ready
        ));
        settings.set_session_delete_consent(1, true, cx);
    });
    host.push(SessionBatchReply::Execute(SessionBatchExecuteResult {
        groups: vec![
            SessionBatchGroupResult {
                items: vec![
                    SessionBatchItem {
                        source_id: "a".into(),
                        status: SessionBatchStatus::Deleted,
                        delete_ids: vec!["a".into(), "a.child".into()],
                        reason: None,
                    },
                    SessionBatchItem::refused(
                        "c".into(),
                        SessionBatchStatus::Skipped,
                        SessionBatchReason::new(SessionBatchReasonKind::Active, "guarded"),
                    ),
                ],
            },
            SessionBatchGroupResult {
                items: vec![SessionBatchItem::refused(
                    "b".into(),
                    SessionBatchStatus::Failed,
                    SessionBatchReason::new(SessionBatchReasonKind::Provider, "native error"),
                )],
            },
        ],
    }));
    settings.update_in(&mut vcx, |settings, _, cx| {
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, host.clone());
        settings.confirm_session_delete(cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if settings.update_in(&mut vcx, |settings, _, _| {
            settings
                .active_settings()
                .and_then(|state| state.sessions.delete.as_ref())
                .is_some_and(|flow| !matches!(flow.phase, SessionDeletePhase::Executing))
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "execute did not land; requests={}",
            host.seen().len()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let requests = host.seen();
    assert_eq!(requests.len(), 2, "prepare then execute");
    let execute = &requests[1];
    let SessionBatchRequest::Execute(execute) = execute else {
        panic!("the second request is the execute");
    };
    assert_eq!(execute.agent, CLIAgent::Hermes);
    assert_eq!(
        execute.groups.len(),
        2,
        "execute keeps the prepare grouping — it cannot widen a store"
    );
    assert_eq!(
        execute.groups[0]
            .confirmations
            .iter()
            .map(|confirmation| confirmation.source_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "c"],
        "group order and row order survive from the selection"
    );
    assert_eq!(
        execute.groups[0].confirmations[0].expected_delete_ids,
        ["a", "a.child"],
        "the confirmation binds the exact cascade prepare computed"
    );
    assert_eq!(
        execute.groups[1].confirmations[0].expected_delete_ids,
        ["b"]
    );

    settings.update_in(&mut vcx, |settings, _, _| {
        let flow = settings
            .active_settings()
            .unwrap()
            .sessions
            .delete
            .as_ref()
            .expect("flow still open while executing");
        assert!(
            matches!(flow.phase, SessionDeletePhase::Done(_)),
            "the canned execute answer landed"
        );
        let SessionDeletePhase::Done(result) = &flow.phase else {
            unreachable!();
        };
        let SessionBatchTotals {
            deleted,
            skipped,
            failed,
        } = result.totals();
        assert_eq!(
            (deleted, skipped, failed),
            (1, 1, 1),
            "partial outcomes aggregate accurately across groups"
        );
        assert_eq!(result.deleted_ids(), ["a", "a.child"]);
    });

    // 收尾清空勾选并触发设置页重扫。
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.finish_session_delete(cx);
        let state = &settings.active_settings().unwrap().sessions;
        assert!(state.delete.is_none(), "the flow closed");
        assert!(state.selected.is_empty(), "the selection cleared");
        assert!(
            state.bound.is_none() && !state.landed,
            "the page rebinds and rescans on the next render"
        );
    });
    vcx.run_until_parked();
    let events = events.borrow();
    let deleted_event = events.iter().find_map(|event| match event {
        crate::ui::settings_window::SettingsEvent::SessionsDeleted { host, removed } => {
            Some((*host, removed.clone()))
        }
        _ => None,
    });
    assert_eq!(
        deleted_event,
        Some((
            HostId::LOCAL,
            vec![(
                session_history::store_group_key(&SessionIdentity::of(HostId::LOCAL, &listed_a)),
                vec!["a".into(), "a.child".into()],
            )]
        )),
        "the workspace learns which native ids actually went away"
    );
}

#[test]
fn removing_a_session_cache_entry_preserves_the_same_id_in_another_store() {
    let host = HostId(u64::MAX - 1);
    let removed = hermes_session("same", "/store/one", "/w");
    let unrelated = hermes_session("same", "/store/two", "/w");
    let store = session_history::store_group_key(&SessionIdentity::of(host, &removed));
    session_history::remember_sessions(host, vec![removed, unrelated.clone()]);
    session_history::forget_sessions(host, &[(store, vec!["same".into()])]);
    let found = session_history::cached_sessions(host);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].scope, unrelated.scope);
    session_history::remember_sessions(host, Vec::new());
}
