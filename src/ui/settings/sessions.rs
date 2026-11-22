//! 设置 → 会话：A 版布局 —— 顶部筛选工具栏，左侧「会话／已隐藏」页签与复选框列表，
//! 右侧常驻单条详情或批量选中清单。发现层复用
//! [`crate::ui::session_history`] 的 plan/collect 与可见身份模型；原生删除
//! 界面归 #367，本页只给出真实的批量选择与不可删除原因接口。
use super::kit::{self, Tk, fs};
use super::shell::{MenuEntry, SearchOption};
use super::*;
use chrono::{Datelike as _, Timelike as _};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::checkbox::Checkbox;
use lyrel_core::core::agent_history::PastSession;
use lyrel_ui_base::host_ops::HostOps;
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::L10nKey;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::core::cli_agent::CLIAgent;
use crate::ui::session_history::{
    self, SessionIdentity, SessionListing, SessionStore, is_session_hidden,
};

/// 左栏页签：Lyrel 列表中的会话，或已隐藏的会话。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionsTab {
    Visible,
    Hidden,
}

/// 「最近活动」筛选。自定义范围由两个日期输入框承载，解析失败的一侧按
/// 不限处理 —— 筛选不能因半个日期未填就把结果清空。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionActivity {
    Any,
    LastSevenDays,
    LastThirtyDays,
    OlderThanThirtyDays,
    Custom,
}

/// 一条已选会话当前不能删除的原因。批量清单把这个原因逐条标在记录旁；
/// 没有原因的记录就是当前已知可删的（真正的删除按钮归 #367）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SessionBlocker {
    /// 本工作区有一个窗格正按同一身份（主机、智能体、存储作用域、原生
    /// ID）运行这条会话。
    Running,
    /// 该来源没有删除路径：OpenCode 会话住在服务存储里，协议尚无删除
    /// 操作。
    SourceUnsupported,
    /// 扫描没有保留下可指认的存储位置，一次精确删除无法证明它删的是什么。
    StoreUnconfirmed,
}

impl SessionBlocker {
    fn label(self) -> gpui_kit::SharedString {
        let key = match self {
            SessionBlocker::Running => L10nKey::SessionsBlockerRunning,
            SessionBlocker::SourceUnsupported => L10nKey::SessionsBlockerSourceUnsupported,
            SessionBlocker::StoreUnconfirmed => L10nKey::SessionsBlockerStoreUnconfirmed,
        };
        t(key).into()
    }
}

/// 设置 → 会话 页的会话状态。一份设置会话持有一份；勾选集只存 key，
/// 作用域永远等于「当前主机 + 当前页签 + 当前筛选」，切换任一即清空。
pub(crate) struct SessionsState {
    /// 一次只看一台主机；初始为发起设置的工作区所在主机。
    pub(crate) host: HostId,
    /// 当前列表所属的主机：`None` 到首次扫描认领前。与 `host` 不一致就
    /// 说明主机刚切走，下一轮 render 会重新扫描。
    bound: Option<HostId>,
    listing: SessionListing,
    generation: u64,
    scanning: bool,
    landed: bool,
    tab: SessionsTab,
    agent: Option<CLIAgent>,
    activity: SessionActivity,
    /// 标题/ID 搜索与自定义范围的两个日期输入；它们的 Edit 事件在
    /// `build_settings_state` 里订阅，任何一次输入都清空勾选。
    pub(crate) start: Entity<InputState>,
    pub(crate) end: Entity<InputState>,
    directory: Option<PathBuf>,
    pub(crate) search: Entity<InputState>,
    /// 勾选行的 key（`SessionIdentity::key` 拼写）。
    selected: BTreeSet<String>,
    /// 行列表的滚动位置；工具栏与页签固定，只有行在滚。
    list_scroll: gpui_kit::ScrollHandle,
    /// 右栏常驻详情对应的会话 key；筛选项变化不影响它，列表里没有了才放下。
    detail: Option<String>,
}

/// 一条列表行渲染所需的全部数据：会话本身、勾选状态、详情常驻状态。
struct SessionRow {
    session: PastSession,
    key: String,
    blocked: Vec<SessionBlocker>,
}

const DAY_SECS: u64 = 86_400;
const SIDE_W: f32 = 300.;
const ROW_H: f32 = 46.;
const AVATAR: f32 = 18.;

impl SessionsState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>, host: HostId) -> Self {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        Self {
            host,
            bound: None,
            listing: SessionListing::default(),
            generation: 0,
            scanning: false,
            landed: false,
            tab: SessionsTab::Visible,
            agent: None,
            activity: SessionActivity::Any,
            start: cx.new(|cx| InputState::new(window, cx).default_value(date.clone())),
            end: cx.new(|cx| InputState::new(window, cx).default_value(date)),
            directory: None,
            search: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t(L10nKey::SessionsSearchPlaceholder))
            }),
            selected: BTreeSet::new(),
            list_scroll: gpui_kit::ScrollHandle::new(),
            detail: None,
        }
    }

    /// 自定义范围的两个边界；解析失败的一侧为 `None`，按不限处理。
    fn custom_window(&self, cx: &App) -> (Option<chrono::NaiveDate>, Option<chrono::NaiveDate>) {
        let parse = |input: &Entity<InputState>| {
            chrono::NaiveDate::parse_from_str(input.read(cx).value().trim(), "%Y-%m-%d").ok()
        };
        (parse(&self.start), parse(&self.end))
    }

    /// 当前主机已发现的全部目录，排序后供目录选择器搜索。
    fn discovered_directories(
        &self,
        home: Option<&Path>,
    ) -> Vec<(PathBuf, gpui_kit::SharedString)> {
        let mut dirs: Vec<PathBuf> = self
            .listing
            .sessions
            .iter()
            .filter_map(|s| s.cwd.clone())
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs.into_iter()
            .map(|dir| {
                let shown = crate::ui::home::display_path(&dir, home).into();
                (dir, shown)
            })
            .collect()
    }

    /// 勾选集的作用域变化（主机、页签、任一筛选）时调用：勾选清空，
    /// 详情随主机走。
    fn clear_scope(&mut self, host_moved: bool) {
        self.selected.clear();
        if host_moved {
            self.detail = None;
        }
    }

    fn select_host(&mut self, host: HostId) {
        if self.host == host {
            return;
        }
        self.host = host;
        self.clear_scope(true);
    }

    fn set_tab(&mut self, tab: SessionsTab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.clear_scope(false);
    }

    fn set_agent(&mut self, agent: Option<CLIAgent>) {
        if self.agent == agent {
            return;
        }
        self.agent = agent;
        self.clear_scope(false);
    }

    fn set_activity(&mut self, activity: SessionActivity) {
        if self.activity == activity {
            return;
        }
        self.activity = activity;
        self.clear_scope(false);
    }

    fn set_directory(&mut self, directory: Option<PathBuf>) {
        if self.directory == directory {
            return;
        }
        self.directory = directory;
        self.clear_scope(false);
    }

    fn set_detail(&mut self, key: Option<String>) {
        self.detail = key;
    }

    /// 勾选一行（或按新状态取消勾选）。
    fn set_row_selected(&mut self, key: String, selected: bool) {
        if selected {
            self.selected.insert(key);
        } else {
            self.selected.remove(&key);
        }
    }

    /// 全选/清空当前筛选结果：三态复选框的点击语义。
    fn set_all_selected(&mut self, keys: Vec<String>, selected: bool) {
        self.selected.clear();
        if selected {
            self.selected.extend(keys);
        }
    }
}

/// 页签与筛选落地后的可见行：隐藏按可见性模型拆分，其余条件依次收紧。
/// 目录比较走所选主机的分隔符语义 —— 客户端平台永远不解析远程路径。
/// 纯函数 —— render 与测试共用同一答案。
#[allow(clippy::too_many_arguments)]
fn filtered_sessions(
    sessions: &[PastSession],
    host: HostId,
    cfg: &Config,
    tab: SessionsTab,
    agent: Option<CLIAgent>,
    activity: SessionActivity,
    directory: Option<&Path>,
    host_separator: char,
    search: &str,
    custom: (Option<chrono::NaiveDate>, Option<chrono::NaiveDate>),
    now: u64,
) -> Vec<PastSession> {
    let query = search.trim().to_lowercase();
    sessions
        .iter()
        .filter(|s| {
            let identity = SessionIdentity::of(host, s);
            match tab {
                SessionsTab::Visible => !is_session_hidden(cfg, &identity),
                SessionsTab::Hidden => is_session_hidden(cfg, &identity),
            }
        })
        .filter(|s| agent.is_none_or(|wanted| s.agent == wanted))
        .filter(|s| {
            directory.is_none_or(|dir| {
                s.cwd.as_deref().is_some_and(|cwd| {
                    crate::core::agent_history::directory_matches_with(cwd, dir, host_separator)
                })
            })
        })
        .filter(|s| match_activity(s.updated, activity, custom, now))
        .filter(|s| {
            query.is_empty()
                || s.title.to_lowercase().contains(&query)
                || s.id.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}

fn match_activity(
    updated: u64,
    activity: SessionActivity,
    custom: (Option<chrono::NaiveDate>, Option<chrono::NaiveDate>),
    now: u64,
) -> bool {
    match activity {
        SessionActivity::Any => true,
        SessionActivity::LastSevenDays => now.saturating_sub(updated) <= 7 * DAY_SECS,
        SessionActivity::LastThirtyDays => now.saturating_sub(updated) <= 30 * DAY_SECS,
        // 未知活动时间（0）不能证明「早于 30 天」。
        SessionActivity::OlderThanThirtyDays => {
            updated > 0 && now.saturating_sub(updated) > 30 * DAY_SECS
        }
        SessionActivity::Custom => {
            let after_start = custom
                .0
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map_or(true, |start| updated >= start.and_utc().timestamp() as u64);
            let before_end = custom
                .1
                .and_then(|d| d.and_hms_opt(23, 59, 59))
                .map_or(true, |end| updated <= end.and_utc().timestamp() as u64);
            after_start && before_end
        }
    }
}

/// 三态复选框的当前状态：全选当前结果 / 部分勾选 / 没有勾选。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SelectAllState {
    All,
    Some,
    None,
}

fn select_all_state(selected: &BTreeSet<String>, rows: &[SessionRow]) -> SelectAllState {
    let checked = rows
        .iter()
        .filter(|row| selected.contains(&row.key))
        .count();
    match checked {
        0 => SelectAllState::None,
        n if n == rows.len() => SelectAllState::All,
        _ => SelectAllState::Some,
    }
}

/// 一条已选会话当前已知的不可删除原因。运行中最优先；OpenCode 的服务
/// 存储没有删除路径；其余来源需要扫描保留下可指认的存储位置。
fn session_blockers(identity: &SessionIdentity, live: &[SessionIdentity]) -> Vec<SessionBlocker> {
    let mut out = Vec::new();
    if live.iter().any(|open| open.same_session(identity)) {
        out.push(SessionBlocker::Running);
    }
    if identity.agent == CLIAgent::OpenCode {
        out.push(SessionBlocker::SourceUnsupported);
    } else if identity.store() == SessionStore::Unconfirmed {
        out.push(SessionBlocker::StoreUnconfirmed);
    }
    out
}

/// 存储来源一行话：有精确文件时写文件的实际位置；否则写它确认到的
/// 提供方存储选择子——OpenCode 的存储选择器或已保存连接，其它来源的
/// 显式 Hermes home、OMP 配置根等非默认 profile／存储根（缺省仍是默认
/// profile）。
fn storage_label(identity: &SessionIdentity, home: Option<&Path>) -> gpui_kit::SharedString {
    match identity.store() {
        SessionStore::File(path) => crate::ui::home::display_path(&path, home).into(),
        SessionStore::Named {
            profile,
            root,
            dir,
            connection,
        } => {
            let address = root
                .or(connection)
                .or_else(|| dir.map(|dir| dir.to_string_lossy().into_owned()));
            match (address, profile.as_deref()) {
                (Some(address), Some(profile)) if profile != "default" => {
                    format!("{profile} · {address}").into()
                }
                (Some(address), _) => address.into(),
                (None, Some(profile)) if profile != "default" => profile.into(),
                _ => t(L10nKey::SessionsDefaultProfile).into(),
            }
        }
        SessionStore::Unconfirmed => t(L10nKey::SessionsDefaultProfile).into(),
    }
}

/// 详情里的绝对活动时间：zh 写「10月18日 15:20」，en 写「Oct 18, 15:20」。
fn absolute_time(updated: u64) -> String {
    const EN_MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(dt) = chrono::DateTime::from_timestamp(updated as i64, 0) else {
        return "—".into();
    };
    let local = dt.with_timezone(&chrono::Local);
    let (year, month, day) = (local.year(), local.month0() as usize, local.day());
    let (hour, minute) = (local.hour(), local.minute());
    if lyrel_ui_base::i18n::current_language_code() == "zh-CN" {
        let _ = year;
        format!("{month}月{day}日 {hour:02}:{minute:02}")
    } else {
        format!("{}, {day:02}, {hour:02}:{minute:02}", EN_MONTHS[month])
    }
}

impl SettingsWindow {
    /// Keep the Sessions page bound to its one host: claimed before the trip,
    /// retired by generation if the host moved while the scan was in flight.
    /// Driven from `Render::render` like the right panel's sync — cheap unless
    /// the bound actually moved.
    pub(crate) fn sync_sessions_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        if settings.navigation.section() != SettingsSection::Sessions
            || settings.navigation.is_searching()
        {
            return;
        }
        let host = settings.sessions.host;
        if settings.sessions.scanning && settings.sessions.bound == Some(host) {
            return;
        }
        if settings.sessions.bound == Some(host) && settings.sessions.landed {
            return;
        }
        // 计划与工作区读取都在认领 bound 之前完成，认领只改自己的状态。
        let shared = HostRegistry::lookup(cx, host);
        let scan = (self.workspace.session_scan)(host, cx);
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        settings.sessions.bound = Some(host);
        settings.sessions.scanning = true;
        settings.sessions.generation += 1;
        let generation = settings.sessions.generation;
        match (shared, scan) {
            (Some(shared), Some(scan)) => {
                HostOps::run_in(
                    shared,
                    window,
                    cx,
                    move |host| session_history::collect_sessions(host, scan),
                    move |this, listing, _window, cx| {
                        this.sessions_settings_landed(generation, host, listing, cx);
                    },
                );
            }
            (None, _) => {
                // 主机在扫描前就没了：直接给出断连的答案，而不是等一趟没有的旅程。
                settings.sessions.listing =
                    SessionListing {
                        sessions: Vec::new(),
                        errors: vec![lyrel_core::host::session_operations::SessionOperationError {
                        kind: lyrel_core::host::session_operations::SessionErrorKind::Connection,
                        message: "sessions settings host is disconnected".into(),
                        retained: None,
                    }],
                        host_reachable: false,
                    };
                settings.sessions.scanning = false;
                settings.sessions.landed = true;
                cx.notify();
            }
            (Some(_), None) => {
                // 工作区不可达（独立测试夹具）：空白落地，页面照常渲染。
                settings.sessions.listing = SessionListing::default();
                settings.sessions.scanning = false;
                settings.sessions.landed = true;
                cx.notify();
            }
        }
    }

    fn sessions_settings_landed(
        &mut self,
        generation: u64,
        host: HostId,
        listing: SessionListing,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        if settings.sessions.generation != generation {
            return;
        }
        session_history::remember_sessions(host, listing.sessions.clone());
        // 勾选与详情只保留列表里还存在的 key：消失的行不能把选择带进死胡同。
        let keys: BTreeSet<String> = listing
            .sessions
            .iter()
            .map(|s| SessionIdentity::of(host, s).key())
            .collect();
        let state = &mut settings.sessions;
        state.selected.retain(|key| keys.contains(key));
        if state.detail.as_ref().is_some_and(|key| !keys.contains(key)) {
            state.detail = None;
        }
        state.listing = listing;
        state.scanning = false;
        state.landed = true;
        cx.notify();
    }

    fn select_sessions_host(&mut self, host: HostId, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.select_host(host);
        }
        cx.notify();
    }

    fn select_sessions_tab(&mut self, tab: SessionsTab, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_tab(tab);
        }
        cx.notify();
    }

    fn select_sessions_agent(&mut self, agent: Option<CLIAgent>, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_agent(agent);
        }
        cx.notify();
    }

    fn select_sessions_activity(&mut self, activity: SessionActivity, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_activity(activity);
        }
        cx.notify();
    }

    fn select_sessions_directory(&mut self, directory: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_directory(directory);
        }
        cx.notify();
    }

    /// 勾选随筛选变化清空 —— 搜索框与自定义日期输入的每一次改动都走这里。
    pub(crate) fn sessions_filter_edited(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.selected.clear();
        }
        cx.notify();
    }

    fn toggle_session_row(&mut self, key: String, selected: bool, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_row_selected(key, selected);
        }
        cx.notify();
    }

    fn toggle_select_all(&mut self, keys: Vec<String>, selected: bool, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_all_selected(keys, selected);
        }
        cx.notify();
    }

    fn open_session_detail(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.set_detail(Some(key));
        }
        cx.notify();
    }

    fn clear_session_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.selected.clear();
        }
        cx.notify();
    }

    /// 在 Lyrel 中隐藏／重新显示：只写可见性偏好，不触碰智能体自己的存储。
    /// 行随勾选状态移动页签，勾选随之清空。作用域键精确到行；重新显示一
    /// 条旧版 `slug:id` 隐藏的行时写它的精确作用域覆盖，而不是删掉旧版
    /// 键 —— 那会连带暴露其它同名行。
    fn set_sessions_hidden(
        &mut self,
        hide: bool,
        identities: Vec<SessionIdentity>,
        cx: &mut Context<Self>,
    ) {
        if identities.is_empty() {
            return;
        }
        self.update_config(cx, |cfg| {
            for identity in identities {
                if hide {
                    session_history::hide_session(cfg, &identity);
                } else {
                    session_history::show_session(cfg, &identity);
                }
            }
        });
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.selected.clear();
        }
        cx.notify();
    }

    /// 页面数据：扫描未落地时先用共享缓存， landed 后才谈健康状态。
    fn sessions_source(&self) -> (Vec<PastSession>, Option<SessionListing>) {
        let Some(settings) = self.active_settings() else {
            return (Vec::new(), None);
        };
        let state = &settings.sessions;
        if state.landed {
            (state.listing.sessions.clone(), Some(state.listing.clone()))
        } else {
            (session_history::cached_sessions(state.host), None)
        }
    }

    pub(crate) fn render_settings_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(settings) = self.active_settings() else {
            return div().into_any_element();
        };
        let tk = Tk::of(cx);
        let cfg = cx.global::<Config>();
        let state = &settings.sessions;
        let host = state.host;
        let now = lyrel_core::core::config::unix_now();
        let home = crate::ui::desktop_lookups::home_for_host(cx, host);
        let live = (self.workspace.live_sessions)(cx);
        let (source, landed_listing) = self.sessions_source();
        let search = state.search.read(cx).value();
        let custom = state.custom_window(cx);
        // The directory filter judges this host's records by this host's
        // path semantics (a remote Linux host stays case-sensitive even
        // under a Windows client), not by the settings window's platform.
        let host_separator = HostRegistry::lookup(cx, host)
            .map_or(std::path::MAIN_SEPARATOR, |host| host.separator());
        let filtered = filtered_sessions(
            &source,
            host,
            cfg,
            state.tab,
            state.agent,
            state.activity,
            state.directory.as_deref(),
            host_separator,
            &search,
            custom,
            now,
        );
        let rows: Vec<SessionRow> = filtered
            .iter()
            .map(|session| {
                let identity = SessionIdentity::of(host, session);
                SessionRow {
                    key: identity.key(),
                    blocked: session_blockers(&identity, &live),
                    session: session.clone(),
                }
            })
            .collect();

        let toolbar = self.sessions_toolbar(state, host, home.as_deref(), cx);
        let mut left = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap(px(8.))
            .child(toolbar);
        if state.activity == SessionActivity::Custom {
            left = left.child(self.sessions_custom_range(state));
        }
        left = left.child(self.sessions_list(
            state,
            &rows,
            landed_listing.as_ref(),
            now,
            home.as_deref(),
            cx,
        ));
        let side = self.sessions_side(state, &rows, host, home.as_deref(), cx);

        h_flex()
            .id("settings-sessions")
            .test_support()
            .w_full()
            .h_full()
            .min_h_0()
            .gap(px(16.))
            .child(left)
            .child(
                div()
                    .id("sessions-side")
                    .test_support()
                    .w(px(SIDE_W))
                    .flex_shrink_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .border_l_1()
                    .border_color(tk.k08)
                    .pl(px(16.))
                    .child(side),
            )
            .into_any_element()
    }

    /// 顶部筛选行：标题/ID 搜索、智能体、最近活动、目录。
    fn sessions_toolbar(
        &self,
        state: &SessionsState,
        host: HostId,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let lead = |key: L10nKey| {
            Some(
                div()
                    .text_size(fs(12.5))
                    .text_color(tk.k45)
                    .child(t(key))
                    .into_any_element(),
            )
        };
        let machines = self.agent_hooks_machines(cx);
        let host_label = machines
            .iter()
            .find(|m| m.host == host)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| format!("{host:?}"));
        let host_entries = machines
            .into_iter()
            .map(|m| {
                MenuEntry::item(m.label.clone(), m.host == host, move |this, _window, cx| {
                    this.select_sessions_host(m.host, cx);
                })
            })
            .collect();
        let host_menu = self.settings_dropdown("sessions-host", host_label, None, host_entries, cx);

        let agent_label = state
            .agent
            .map(|agent| agent.display_name())
            .unwrap_or_else(|| t(L10nKey::SessionsAllAgents));
        let agent_entries = std::iter::once((None, t(L10nKey::SessionsAllAgents)))
            .chain(
                CLIAgent::ALL
                    .into_iter()
                    .map(|agent| (Some(agent), agent.display_name())),
            )
            .map(|(agent, label)| {
                MenuEntry::item(label, state.agent == agent, move |this, _window, cx| {
                    this.select_sessions_agent(agent, cx);
                })
            })
            .collect();
        let agent_menu =
            self.settings_dropdown("sessions-agent", agent_label, None, agent_entries, cx);

        let activity_key = match state.activity {
            SessionActivity::Any => L10nKey::SessionsActivityAny,
            SessionActivity::LastSevenDays => L10nKey::SessionsActivityLastSevenDays,
            SessionActivity::LastThirtyDays => L10nKey::SessionsActivityLastThirtyDays,
            SessionActivity::OlderThanThirtyDays => L10nKey::SessionsActivityOlderThirtyDays,
            SessionActivity::Custom => L10nKey::SessionsActivityCustom,
        };
        let activity_entries = [
            (SessionActivity::Any, L10nKey::SessionsActivityAny),
            (
                SessionActivity::LastSevenDays,
                L10nKey::SessionsActivityLastSevenDays,
            ),
            (
                SessionActivity::LastThirtyDays,
                L10nKey::SessionsActivityLastThirtyDays,
            ),
            (
                SessionActivity::OlderThanThirtyDays,
                L10nKey::SessionsActivityOlderThirtyDays,
            ),
            (SessionActivity::Custom, L10nKey::SessionsActivityCustom),
        ]
        .into_iter()
        .map(|(activity, key)| {
            MenuEntry::item(
                t(key),
                state.activity == activity,
                move |this, _window, cx| {
                    this.select_sessions_activity(activity, cx);
                },
            )
        })
        .collect();
        let activity_menu = self.settings_dropdown(
            "sessions-activity",
            t(activity_key),
            lead(L10nKey::SessionsActivityLead),
            activity_entries,
            cx,
        );

        let dirs = state.discovered_directories(home);
        let current_dir = state
            .directory
            .as_ref()
            .and_then(|wanted| dirs.iter().position(|(dir, _)| dir == wanted));
        let dir_label: gpui_kit::SharedString = state
            .directory
            .as_ref()
            .map(|dir| crate::ui::home::display_path(dir, home).into())
            .unwrap_or_else(|| t(L10nKey::SessionsAllDirectories).into());
        let options = Rc::new(
            dirs.iter()
                .map(|(_, shown)| SearchOption {
                    label: shown.clone(),
                    font: None,
                })
                .collect::<Vec<_>>(),
        );
        let pick_dirs: Rc<Vec<PathBuf>> =
            Rc::new(dirs.iter().map(|(dir, _)| dir.clone()).collect());
        let picked = pick_dirs.clone();
        let directory_menu = self.settings_search_dropdown(
            "sessions-directory",
            dir_label,
            options,
            current_dir,
            Rc::new(move |this, ix, _window, cx| {
                let dir = picked.get(ix).cloned();
                this.select_sessions_directory(dir, cx);
            }),
            cx,
        );

        h_flex()
            .id("sessions-toolbar")
            .test_support()
            .w_full()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .child(kit::search_field(&state.search, &tk)),
            )
            .child(host_menu)
            .child(agent_menu)
            .child(activity_menu)
            .child(directory_menu)
            .into_any_element()
    }

    /// 自定义范围的两个日期输入，改动即收紧筛选（勾选已随改动清空）。
    fn sessions_custom_range(&self, state: &SessionsState) -> AnyElement {
        h_flex()
            .id("sessions-custom-range")
            .test_support()
            .items_center()
            .gap_2()
            .child(
                Input::new(&state.start)
                    .id("sessions-start-date")
                    .aria_label(t(L10nKey::StatisticsStartDate))
                    .small(),
            )
            .child(
                Input::new(&state.end)
                    .id("sessions-end-date")
                    .aria_label(t(L10nKey::StatisticsEndDate))
                    .small(),
            )
            .into_any_element()
    }

    /// 左栏：页签 + 计数、三态全选、行列表或对应空状态。
    fn sessions_list(
        &self,
        state: &SessionsState,
        rows: &[SessionRow],
        landed: Option<&SessionListing>,
        now: u64,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let mut head = h_flex().items_center().gap(px(8.));
        for (tab, key) in [
            (SessionsTab::Visible, L10nKey::SettingsNavSessions),
            (SessionsTab::Hidden, L10nKey::SessionsTabHidden),
        ] {
            let active = state.tab == tab;
            head = head.child(
                div()
                    .id(format!(
                        "sessions-tab-{}",
                        match tab {
                            SessionsTab::Visible => "visible",
                            SessionsTab::Hidden => "hidden",
                        }
                    ))
                    .test_support()
                    .cursor_pointer()
                    .text_size(fs(13.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if active { tk.fg } else { tk.k45 })
                    .hover(move |s| s.text_color(tk.fg))
                    .child(t(key))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_sessions_tab(tab, cx);
                    })),
            );
        }
        head = head.child(
            div()
                .flex_1()
                .text_size(fs(12.))
                .text_color(tk.k45)
                .text_right()
                .child(t_fmt(
                    L10nKey::SessionsCountFmt,
                    &[("count", &rows.len().to_string())],
                )),
        );

        let all_state = select_all_state(&state.selected, rows);
        let row_keys: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
        let toggle_keys = row_keys.clone();
        let select_all = h_flex()
            .id("sessions-select-all")
            .test_support()
            .h(px(28.))
            .items_center()
            .gap(px(6.))
            .child(
                Checkbox::new("sessions-select-all-check")
                    .checked(all_state == SelectAllState::All)
                    .indeterminate(all_state == SelectAllState::Some)
                    .small()
                    .on_click(cx.listener(move |this, checked, _window, cx| {
                        this.toggle_select_all(toggle_keys.clone(), *checked, cx);
                    })),
            )
            .child(
                div()
                    .text_size(fs(12.))
                    .text_color(tk.k45)
                    .child(t(L10nKey::SessionsSelectAll)),
            );

        let mut list = v_flex().w_full().min_h_0().gap(px(2.));
        for (ix, row) in rows.iter().enumerate() {
            list = list.child(self.session_row(state, ix, row, now, home, cx));
        }
        let body: AnyElement = match (rows.is_empty(), landed) {
            (true, None) => self.sessions_empty_notice(t(L10nKey::SessionsScanning), None, &tk),
            (true, Some(listing)) => match listing.health(0) {
                crate::ui::session_history::SessionHealth::Disconnected => self
                    .sessions_empty_notice(
                        t(L10nKey::PanelSessionsDisconnected),
                        Some(t(L10nKey::PanelSessionsDisconnectedHint).into()),
                        &tk,
                    ),
                crate::ui::session_history::SessionHealth::Unsupported => self
                    .sessions_empty_notice(
                        t(L10nKey::PanelSessionsUnsupported),
                        Some(t(L10nKey::PanelSessionsUnsupportedHint).into()),
                        &tk,
                    ),
                _ => {
                    let title = match state.tab {
                        SessionsTab::Visible => t(L10nKey::SessionsEmpty),
                        SessionsTab::Hidden => t(L10nKey::SessionsEmptyHidden),
                    };
                    self.sessions_empty_notice(title, None, &tk)
                }
            },
            (false, _) => div()
                .id("sessions-rows")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&state.list_scroll)
                .child(list)
                .into_any_element(),
        };
        let mut col = v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .gap(px(4.))
            .child(head)
            .child(select_all);
        if let Some(listing) = landed
            && listing.health(rows.len()) == crate::ui::session_history::SessionHealth::Partial
        {
            col = col.child(
                div()
                    .text_size(fs(11.5))
                    .text_color(tk.k45)
                    .child(t(L10nKey::PanelSessionsPartial)),
            );
        }
        col.child(body).into_any_element()
    }

    fn sessions_empty_notice(
        &self,
        title: impl Into<gpui_kit::SharedString>,
        hint: Option<gpui_kit::SharedString>,
        tk: &Tk,
    ) -> AnyElement {
        let mut note = v_flex().w_full().py(px(24.)).gap(px(4.)).child(
            div()
                .text_size(fs(12.5))
                .text_color(tk.k45)
                .child(title.into()),
        );
        if let Some(hint) = hint {
            note = note.child(div().text_size(fs(11.5)).text_color(tk.k35).child(hint));
        }
        note.into_any_element()
    }

    /// 一行：常驻复选框 + 智能体圆盘 + 标题/副标题（点击标题看详情）+ 相对时间。
    fn session_row(
        &self,
        state: &SessionsState,
        ix: usize,
        row: &SessionRow,
        now: u64,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let key = row.key.clone();
        let checked = state.selected.contains(&key);
        let detailed = state.detail.as_deref() == Some(key.as_str());
        let title = row.session.title.clone();
        let subtitle = {
            let dir = row
                .session
                .cwd
                .as_deref()
                .map(|dir| crate::ui::home::display_path(dir, home))
                .unwrap_or_else(|| "—".into());
            format!("{} · {dir}", row.session.agent.display_name())
        };
        let detail_key = key.clone();
        h_flex()
            .id(format!("sessions-row-{ix}"))
            .test_support()
            .w_full()
            .h(px(ROW_H))
            .px(px(6.))
            .gap(px(8.))
            .items_center()
            .rounded(px(6.))
            .when(detailed, |r| r.bg(tk.k05))
            .child(
                Checkbox::new(format!("sessions-check-{ix}"))
                    .checked(checked)
                    .small()
                    .on_click(cx.listener(move |this, on, _window, cx| {
                        this.toggle_session_row(key.clone(), *on, cx);
                    })),
            )
            .child(crate::ui::tab_strip::avatar(
                format!("sessions-avatar-{ix}"),
                crate::ui::search::Avatar {
                    agent: Some(row.session.agent),
                    ..Default::default()
                },
                AVATAR,
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        div()
                            .id(format!("sessions-title-{ix}"))
                            .test_support()
                            .min_w_0()
                            .truncate()
                            .text_size(fs(12.5))
                            .font_weight(if detailed {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(tk.fg)
                            .cursor_pointer()
                            .child(title)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.open_session_detail(detail_key.clone(), cx);
                            })),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(fs(11.))
                            .text_color(tk.k45)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(fs(11.))
                    .text_color(tk.k45)
                    .child(crate::ui::home::relative_time(now, row.session.updated)),
            )
            .into_any_element()
    }

    /// 右栏：有勾选时是批量清单，否则是常驻单条详情，再没有就是提示。
    fn sessions_side(
        &self,
        state: &SessionsState,
        rows: &[SessionRow],
        host: HostId,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !state.selected.is_empty() {
            return self.sessions_selection(state, rows, host, cx);
        }
        let by_key: std::collections::HashMap<&str, &SessionRow> =
            rows.iter().map(|row| (row.key.as_str(), row)).collect();
        let detail = state
            .detail
            .as_deref()
            .and_then(|key| by_key.get(key).copied());
        match detail {
            Some(row) => self.sessions_detail(state, row, host, home, cx),
            None => {
                let tk = Tk::of(cx);
                div()
                    .id("sessions-side-empty")
                    .test_support()
                    .pt(px(48.))
                    .text_size(fs(12.))
                    .text_color(tk.k35)
                    .child(t(L10nKey::SessionsDetailEmpty))
                    .into_any_element()
            }
        }
    }

    /// 常驻详情：标题 + 智能体/工作目录/最近活动/存储来源/会话 ID，页脚
    /// 给出 reversible 的隐藏动作。删除按钮与确认归 #367，不在此页出现。
    fn sessions_detail(
        &self,
        state: &SessionsState,
        row: &SessionRow,
        host: HostId,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let session = &row.session;
        let identity = SessionIdentity::of(host, session);
        let hidden = is_session_hidden(cx.global::<Config>(), &identity);
        let directory = session
            .cwd
            .as_deref()
            .map(|dir| crate::ui::home::display_path(dir, home))
            .unwrap_or_else(|| "—".into());
        let session_id = session.id.clone();
        let mono_family = kit::Tk::mono(cx);
        let field = |label: L10nKey, value: gpui_kit::SharedString, mono: bool| {
            h_flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_size(fs(11.5))
                        .text_color(tk.k45)
                        .child(t(label)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(fs(12.))
                        .when(mono, |d| d.font_family(mono_family.clone()))
                        .child(value),
                )
        };
        let hide_label = match (state.tab, hidden) {
            (_, true) => t(L10nKey::SessionsUnhide),
            _ => t(L10nKey::SessionsHide),
        };
        let hide_identities = vec![SessionIdentity::of(host, session)];
        let hide = kit::button(
            "sessions-hide",
            hide_label,
            if hidden {
                kit::BtnKind::Secondary
            } else {
                kit::BtnKind::Primary
            },
        )
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.set_sessions_hidden(!hidden, hide_identities.clone(), cx);
        }));
        v_flex()
            .id("sessions-detail")
            .test_support()
            .w_full()
            .min_h_0()
            .gap(px(12.))
            .child(
                div()
                    .text_size(fs(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(session.title.clone()),
            )
            .child(
                v_flex()
                    .gap(px(8.))
                    .child(field(
                        L10nKey::SessionsDetailAgent,
                        session.agent.display_name().into(),
                        false,
                    ))
                    .child(field(
                        L10nKey::SessionsDetailDirectory,
                        directory.into(),
                        true,
                    ))
                    .child(field(
                        L10nKey::SessionsDetailLastActive,
                        absolute_time(session.updated).into(),
                        false,
                    ))
                    .child(field(
                        L10nKey::SessionsDetailStorage,
                        storage_label(&identity, home),
                        true,
                    ))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(64.))
                                    .flex_none()
                                    .text_size(fs(11.5))
                                    .text_color(tk.k45)
                                    .child(t(L10nKey::SessionsDetailSessionId)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(fs(12.))
                                    .font_family(kit::Tk::mono(cx))
                                    .child(session_id.clone()),
                            )
                            .child(
                                kit::button(
                                    "sessions-copy-id",
                                    t(L10nKey::SessionsCopyId),
                                    kit::BtnKind::Link,
                                )
                                .on_click(cx.listener(
                                    move |_, _, _window, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                            session_id.clone(),
                                        ));
                                    },
                                )),
                            ),
                    ),
            )
            .child(h_flex().pt(px(4.)).child(hide))
            .into_any_element()
    }

    /// 批量清单：已选数量、暂不可删除数量与逐条原因、批量隐藏/重新显示。
    /// 这是 #367 删除确认消费的选择与资格接口 —— 本页不渲染删除按钮。
    fn sessions_selection(
        &self,
        state: &SessionsState,
        rows: &[SessionRow],
        host: HostId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let selected_rows: Vec<&SessionRow> = rows
            .iter()
            .filter(|row| state.selected.contains(&row.key))
            .collect();
        let blocked = selected_rows
            .iter()
            .filter(|row| !row.blocked.is_empty())
            .count();
        let mut body = v_flex()
            .id("sessions-selection")
            .test_support()
            .w_full()
            .min_h_0()
            .gap(px(10.))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(fs(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t_fmt(
                                L10nKey::SessionsSelectedCountFmt,
                                &[("count", &selected_rows.len().to_string())],
                            )),
                    )
                    .child(
                        kit::button(
                            "sessions-cancel-selection",
                            t(L10nKey::SessionsCancelSelection),
                            kit::BtnKind::Link,
                        )
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.clear_session_selection(cx);
                            },
                        )),
                    ),
            );
        if blocked > 0 {
            body = body.child(
                h_flex()
                    .id("sessions-blocked-note")
                    .test_support()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(6.)).rounded_full().bg(tk.warn))
                    .child(
                        div()
                            .text_size(fs(11.5))
                            .text_color(tk.warn_text)
                            .child(t_fmt(
                                L10nKey::SessionsBlockedCountFmt,
                                &[("count", &blocked.to_string())],
                            )),
                    ),
            );
        }
        let mut list = v_flex().gap(px(2.));
        for (ix, row) in selected_rows.iter().enumerate() {
            list = list.child(
                h_flex()
                    .id(format!("sessions-selected-{ix}"))
                    .test_support()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(fs(12.))
                            .child(row.session.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(fs(11.))
                            .text_color(tk.k45)
                            .child(
                                row.blocked
                                    .first()
                                    .map(|blocker| blocker.label())
                                    .unwrap_or_default(),
                            ),
                    ),
            );
        }
        let hide_label = match state.tab {
            SessionsTab::Visible => t(L10nKey::SessionsHide),
            SessionsTab::Hidden => t(L10nKey::SessionsUnhide),
        };
        let identities: Vec<SessionIdentity> = selected_rows
            .iter()
            .map(|row| SessionIdentity::of(host, &row.session))
            .collect();
        let hiding = match state.tab {
            SessionsTab::Visible => true,
            SessionsTab::Hidden => false,
        };
        let hide = kit::button("sessions-hide-selected", hide_label, kit::BtnKind::Primary)
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.set_sessions_hidden(hiding, identities.clone(), cx);
            }));
        body.child(list)
            .child(h_flex().pt(px(4.)).child(hide))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
