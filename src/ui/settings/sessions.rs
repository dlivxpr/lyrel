//! 设置 → 会话：A 版布局 —— 顶部筛选工具栏，左侧「会话／已隐藏」页签与复选框列表，
//! 右侧常驻单条详情或批量选中清单。发现层复用
//! [`crate::ui::session_history`] 的 plan/collect 与可见身份模型；原生删除
//! 走 Host 契约的批量 prepare/execute（#367 起 Hermes 可删，#368–370 复用
//! 同一对契约），本页负责真实选择、逐条原因、确认清单与结果呈现。
use super::kit::{self, Tk, fs};
use super::shell::MenuEntry;
use super::*;
use chrono::{Datelike as _, Timelike as _};
use directory_browser::SessionDirectoryBrowser;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::checkbox::Checkbox;
use lyrel_core::core::agent_history::PastSession;
use lyrel_core::host::session_operations::batch::{
    SessionBatchConfirmation, SessionBatchExecute, SessionBatchExecuteGroup,
    SessionBatchExecuteResult, SessionBatchItem, SessionBatchPrepare, SessionBatchPrepareGroup,
    SessionBatchReasonKind, SessionBatchReply, SessionBatchRequest, SessionBatchStatus,
};
use lyrel_ui_base::host_ops::HostOps;
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::L10nKey;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

mod directory_browser;

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
    /// 扫描没有保留下可指认的存储位置，一次精确删除无法证明它删的是什么。
    StoreUnconfirmed,
    /// OpenCode v2.2.0：原生删除会打断活动会话，且递归级联不提供可确认
    /// 的目标集 —— 在上游给出预期集合围栏前保持不可资格。
    OpenCodeCascadeUnfenced,
    /// Kimi Code ACP v2.6：删除有租约但可能放行、且没有预期身份围栏，
    /// 无法证明删的就是确认的那条。
    KimiDeleteUnfenced,
    /// pi v1.4/1.6：原生删除没有跨进程写入租约，活动写入者无法被证明。
    PiNoWriteLease,
    /// Oh My Pi v19.1：原生 drop 没有写入租约，递归产物范围随配置变化。
    OmpNoWriteLease,
}

impl SessionBlocker {
    fn label(self) -> gpui_kit::SharedString {
        let key = match self {
            SessionBlocker::Running => L10nKey::SessionsBlockerRunning,
            SessionBlocker::StoreUnconfirmed => L10nKey::SessionsBlockerStoreUnconfirmed,
            SessionBlocker::OpenCodeCascadeUnfenced => L10nKey::SessionsBlockerOpenCodeCascade,
            SessionBlocker::KimiDeleteUnfenced => L10nKey::SessionsBlockerKimiUnfenced,
            SessionBlocker::PiNoWriteLease => L10nKey::SessionsBlockerPiNoLease,
            SessionBlocker::OmpNoWriteLease => L10nKey::SessionsBlockerOmpNoLease,
        };
        t(key).into()
    }
}

/// 设置 → 会话 页的会话状态。一份设置会话持有一份；勾选集只存 key，
/// 作用域永远等于「当前主机 + 当前页签 + 当前筛选」，切换任一即清空。
pub(crate) struct SessionsState {
    /// 一次只看一台主机；设置页永远从本机开始，不管它是从哪个工作区
    /// 打开的 —— 本页主机选择只在本次设置会话内保留。
    pub(crate) host: HostId,
    /// 当前列表所属的主机：`None` 到首次扫描认领前。与 `host` 不一致就
    /// 说明主机刚切走，下一轮 render 会重新扫描。
    bound: Option<HostId>,
    connection: Option<lyrel_ui_base::host_ops::SharedHost>,
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
    /// SSH 主机的只读目录浏览弹层；本机挑目录走系统文件夹选择器，不
    /// 留状态。随主机切换取消。
    directory_browser: Option<SessionDirectoryBrowser>,
    /// 浏览器旅程令牌的单调来源：每次打开与每次导航 +1，旧令
    /// 牌永不再用 —— 取消再开不会让旧旅程的数字复活。
    directory_browser_seq: u64,
    pub(crate) search: Entity<InputState>,
    /// 勾选行的 key（`SessionIdentity::key` 拼写）。
    selected: BTreeSet<String>,
    /// 行列表的滚动位置；工具栏与页签固定，只有行在滚。
    list_scroll: gpui_kit::ScrollHandle,
    /// 右栏常驻详情对应的会话 key；筛选项变化不影响它，列表里没有了才放下。
    detail: Option<String>,
    /// 进行中的批量原生删除（确认清单/执行中/结果）；选择或筛选变化时
    /// 由 `clear_scope` 连带取消。
    delete: Option<SessionDeleteFlow>,
}

/// 一条列表行渲染所需的全部数据：会话本身、勾选状态、详情常驻状态。
struct SessionRow {
    session: PastSession,
    key: String,
    blocked: Vec<SessionBlocker>,
}

/// 删除确认清单里的一行：选中顺序保留，组号指回 `groups`，prepare 后
/// 填上原生核验结果。被排除的行（客户端已知不可删）不带组号。
struct SessionDeleteRow {
    identity: SessionIdentity,
    title: gpui_kit::SharedString,
    group: Option<usize>,
    excluded: Option<SessionBlocker>,
    /// Prepare 的原生答案。
    plan: Option<SessionDeletePlan>,
}

/// Prepare 对单行的核验结果。
#[derive(Clone)]
enum SessionDeletePlan {
    /// 存储身份与级联已重新核验；`delete_ids` 是确认清单里要展示的
    /// 精确原生影响范围，Execute 也只允许删这个集合。
    Eligible { delete_ids: Vec<String> },
    /// 原生侧给出的不可删原因。
    Ineligible {
        kind: SessionBatchReasonKind,
        message: gpui_kit::SharedString,
    },
}

/// 一个待删除的存储组：组内所有行共享同一原生存储身份（agent、
/// 主机、scope、cwd、launch argv），整组只发一个 prepare/execute 组。
struct SessionDeleteGroup {
    cwd: PathBuf,
    launch_argv: Vec<String>,
    scope: lyrel_core::host::session_operations::SessionScope,
    /// 组在清单里的展示名：确认到的存储位置。
    store_label: gpui_kit::SharedString,
}

/// 批量删除的相位机。Preparing/Executing 期间窗口仍可渲染，但所有
/// 按钮都不可再点；Done 之后由用户点「完成」收尾刷新。
enum SessionDeletePhase {
    Preparing,
    Ready,
    Failed(gpui_kit::SharedString),
    Executing,
    Done(SessionBatchExecuteResult),
}

/// 一次批量删除的全部状态；`None` 表示没有进行中的删除。
pub(crate) struct SessionDeleteFlow {
    rows: Vec<SessionDeleteRow>,
    groups: Vec<SessionDeleteGroup>,
    host: lyrel_core::host::SharedHost,
    phase: SessionDeletePhase,
    /// 桌面同意清单：两项都勾选才允许按下确认删除。
    consent_irreversible: bool,
    consent_checked: bool,
}

impl SessionDeleteFlow {
    fn eligible(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row.plan, Some(SessionDeletePlan::Eligible { .. })))
            .count()
    }

    fn can_confirm(&self) -> bool {
        matches!(self.phase, SessionDeletePhase::Ready)
            && self.consent_irreversible
            && self.consent_checked
            && self.eligible() > 0
    }

    fn exclude_open_sessions(&mut self, live: &[SessionIdentity]) -> bool {
        if live.is_empty() {
            return false;
        }
        let mut changed = false;
        for row in &mut self.rows {
            let Some(SessionDeletePlan::Eligible { delete_ids }) = &row.plan else {
                continue;
            };
            let store = session_history::store_group_key(&row.identity);
            if live.iter().any(|open| {
                open.host == row.identity.host
                    && open.agent == row.identity.agent
                    && delete_ids.iter().any(|id| open.id == *id)
                    && (open.store() == SessionStore::Unconfirmed
                        || session_history::store_group_key(open) == store)
            }) {
                row.excluded = Some(SessionBlocker::Running);
                row.plan = None;
                changed = true;
            }
        }
        changed
    }
}

const DAY_SECS: u64 = 86_400;
const SIDE_W: f32 = 300.;
const ROW_H: f32 = 46.;
const AVATAR: f32 = 18.;

impl SessionsState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        Self {
            host: HostId::LOCAL,
            bound: None,
            connection: None,
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
            directory_browser: None,
            directory_browser_seq: 0,
            search: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t(L10nKey::SessionsSearchPlaceholder))
            }),
            selected: BTreeSet::new(),
            list_scroll: gpui_kit::ScrollHandle::new(),
            detail: None,
            delete: None,
        }
    }

    /// 自定义范围的两个边界；解析失败的一侧为 `None`，按不限处理。
    fn custom_window(&self, cx: &App) -> (Option<chrono::NaiveDate>, Option<chrono::NaiveDate>) {
        let parse = |input: &Entity<InputState>| {
            chrono::NaiveDate::parse_from_str(input.read(cx).value().trim(), "%Y-%m-%d").ok()
        };
        (parse(&self.start), parse(&self.end))
    }

    /// 勾选集的作用域变化（主机、页签、任一筛选）时调用：勾选清空，
    /// 详情随主机走；进行中的删除清单一并取消 —— 它绑定的就是旧作用域
    /// 里的选中集。
    fn clear_scope(&mut self, host_moved: bool) {
        self.selected.clear();
        self.delete = None;
        if host_moved {
            self.detail = None;
        }
    }

    fn select_host(&mut self, host: HostId) {
        if self.host == host {
            return;
        }
        self.host = host;
        self.directory = None;
        // 浏览器绑定的是旧主机：连进行中的列目录一起作废（代际失效）。
        self.directory_browser = None;
        self.reset_listing();
        self.clear_scope(true);
    }

    fn reset_listing(&mut self) {
        self.bound = None;
        self.connection = None;
        self.listing = SessionListing::default();
        self.generation += 1;
        self.scanning = false;
        self.landed = false;
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
        SessionActivity::Custom => matches_calendar_dates(updated, custom, &chrono::Local),
    }
}

fn matches_calendar_dates(
    updated: u64,
    (start, end): (Option<chrono::NaiveDate>, Option<chrono::NaiveDate>),
    timezone: &impl chrono::TimeZone,
) -> bool {
    let date = i64::try_from(updated)
        .ok()
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|utc| utc.with_timezone(timezone).date_naive());
    date.is_some_and(|date| {
        start.is_none_or(|start| date >= start) && end.is_none_or(|end| date <= end)
    })
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

/// 一条已选会话当前已知的不可删除原因。运行中最优先。只有 Hermes 已有
/// 经验证的原生删除路径（#367）：预期目标集在原生写事务内围栏、活动写
/// 入租约在同一事务里拒绝。其余来源各自的原生删除目前都缺一项可验证
/// 保证（见各变体注释），在各自的适配器把缺口补上之前保持显式不可资
/// 格 —— 没有一个「通用写回退」会把不可验证的来源伪装成可删。扫描没
/// 保留下可指认存储位置的 Hermes 行再补「无法确认存储来源」。
fn session_blockers(identity: &SessionIdentity, live: &[SessionIdentity]) -> Vec<SessionBlocker> {
    let mut out = Vec::new();
    if live.iter().any(|open| open.same_session(identity)) {
        out.push(SessionBlocker::Running);
    }
    match identity.agent {
        CLIAgent::Hermes => {
            if identity.store() == SessionStore::Unconfirmed {
                out.push(SessionBlocker::StoreUnconfirmed);
            }
        }
        CLIAgent::OpenCode => out.push(SessionBlocker::OpenCodeCascadeUnfenced),
        CLIAgent::Kimi => out.push(SessionBlocker::KimiDeleteUnfenced),
        CLIAgent::Pi => out.push(SessionBlocker::PiNoWriteLease),
        CLIAgent::OhMyPi => out.push(SessionBlocker::OmpNoWriteLease),
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
        let shared = HostRegistry::lookup(cx, host).filter(|host| host.is_connected());
        let same_connection = match (&settings.sessions.connection, &shared) {
            (Some(previous), Some(current)) => std::sync::Arc::ptr_eq(previous, current),
            (None, None) => true,
            _ => false,
        };
        if settings.sessions.bound == Some(host)
            && same_connection
            && (settings.sessions.scanning || settings.sessions.landed)
        {
            return;
        }
        let scan = shared
            .as_ref()
            .and_then(|_| (self.workspace.session_scan)(host, cx));
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        settings.sessions.reset_listing();
        settings.sessions.selected.clear();
        settings.sessions.detail = None;
        settings.sessions.connection = shared.clone();
        settings.sessions.bound = Some(host);
        settings.sessions.scanning = true;
        let generation = settings.sessions.generation;
        let session = settings.navigation.session();
        match (shared, scan) {
            (Some(shared), Some(scan)) => {
                let owner = shared.clone();
                HostOps::run_in(
                    shared,
                    window,
                    cx,
                    move |host| session_history::collect_sessions(host, scan),
                    move |this, listing, _window, cx| {
                        if HostRegistry::lookup(cx, host).is_some_and(|current| {
                            current.is_connected() && std::sync::Arc::ptr_eq(&owner, &current)
                        }) {
                            this.sessions_settings_landed(session, generation, host, listing, cx);
                        }
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
                        any_listed: false,
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
        session: gpui_kit::EntityId,
        generation: u64,
        host: HostId,
        listing: SessionListing,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        if !settings.navigation.accepts(session)
            || settings.sessions.generation != generation
            || settings.sessions.host != host
        {
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
}

/// 一次删除按钮点击携带的候选行：选中顺序、可见身份与客户端已知
/// 的不可删原因。
#[derive(Clone)]
pub(crate) struct SessionDeleteCandidate {
    identity: SessionIdentity,
    title: gpui_kit::SharedString,
    blocked: Vec<SessionBlocker>,
    cwd: Option<PathBuf>,
}

/// 把候选行分成「精确原生存储组」：组键是 agent＋主机＋存储选择子，
/// 同组的行共享一个 prepare/execute 组，顺序保持选中的先后；带阻
/// 挡原因的行进清单但不进任何组。launch 上下文优先复用本次扫描
/// 计划的同一请求（同一可执行文件与同一 home/profile），扫描不
/// 可达时退回该 agent 的默认启动命令。
fn build_delete_plan(
    candidates: &[SessionDeleteCandidate],
    scan: Option<&session_history::SessionScan>,
    home: Option<&Path>,
    agent_launch: &std::collections::HashMap<String, String>,
) -> (Vec<SessionDeleteRow>, Vec<SessionDeleteGroup>) {
    use std::collections::HashMap;
    let mut rows = Vec::with_capacity(candidates.len());
    let mut groups: Vec<SessionDeleteGroup> = Vec::new();
    let mut group_ix: HashMap<String, usize> = HashMap::new();
    for candidate in candidates {
        if let Some(blocker) = candidate.blocked.first().copied() {
            rows.push(SessionDeleteRow {
                identity: candidate.identity.clone(),
                title: candidate.title.clone(),
                group: None,
                excluded: Some(blocker),
                plan: None,
            });
            continue;
        }
        let agent = candidate.identity.agent;
        let scope = candidate.identity.scope.clone().unwrap_or_default();
        let ix = *group_ix
            .entry(session_history::store_group_key(&candidate.identity))
            .or_insert_with(|| {
                let (cwd, launch_argv, scope) = scan
                    .and_then(|scan| scan.batch_context(agent, &scope))
                    .unwrap_or_else(|| {
                        let cwd = candidate
                            .cwd
                            .clone()
                            .or_else(|| home.map(|home| home.to_path_buf()))
                            .unwrap_or_default();
                        let launch_argv = crate::core::cli_agent::parse_command_argv(
                            &agent.launch_command(agent_launch),
                        )
                        .unwrap_or_default();
                        (cwd, launch_argv, scope.clone())
                    });
                groups.push(SessionDeleteGroup {
                    cwd,
                    launch_argv,
                    scope,
                    store_label: storage_label(&candidate.identity, home),
                });
                groups.len() - 1
            });
        rows.push(SessionDeleteRow {
            identity: candidate.identity.clone(),
            title: candidate.title.clone(),
            group: Some(ix),
            excluded: None,
            plan: None,
        });
    }
    (rows, groups)
}

impl SettingsWindow {
    /// 只显示当前连接已经返回的列表。旧连接的缓存不参与选择或删除。
    fn sessions_source(&self) -> (Vec<PastSession>, Option<SessionListing>) {
        let Some(settings) = self.active_settings() else {
            return (Vec::new(), None);
        };
        let state = &settings.sessions;
        if state.landed {
            (state.listing.sessions.clone(), Some(state.listing.clone()))
        } else {
            (Vec::new(), None)
        }
    }

    /// 删除按钮按下：把当前选中分成精确存储组，先在所属主机上
    /// prepare（重新核验存储身份、活动写入与原生级联），再打开确认
    /// 清单。客户端已知不可删的行只进清单不进组。
    fn start_session_delete(
        &mut self,
        window: &mut Window,
        candidates: Vec<SessionDeleteCandidate>,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings() else {
            return;
        };
        let host = settings.sessions.host;
        let home = crate::ui::desktop_lookups::home_for_host(cx, host);
        let scan = (self.workspace.session_scan)(host, cx);
        let agent_launch = cx.global::<Config>().agent_launch.clone();
        let (rows, groups) =
            build_delete_plan(&candidates, scan.as_ref(), home.as_deref(), &agent_launch);
        if groups.is_empty() {
            return;
        }
        let Some(shared) = HostRegistry::lookup(cx, host) else {
            return;
        };
        let wire_groups: Vec<SessionBatchPrepareGroup> = groups
            .iter()
            .enumerate()
            .map(|(ix, group)| SessionBatchPrepareGroup {
                cwd: group.cwd.clone(),
                launch_argv: group.launch_argv.clone(),
                scope: group.scope.clone(),
                source_ids: rows
                    .iter()
                    .filter(|row| row.group == Some(ix))
                    .map(|row| row.identity.id.clone())
                    .collect(),
            })
            .collect();
        let request = SessionBatchRequest::Prepare(Box::new(SessionBatchPrepare {
            expected_host: host.0,
            agent: CLIAgent::Hermes,
            groups: wire_groups,
        }));
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        settings.sessions.delete = Some(SessionDeleteFlow {
            rows,
            groups,
            host: shared.clone(),
            phase: SessionDeletePhase::Preparing,
            consent_irreversible: false,
            consent_checked: false,
        });
        HostOps::run_in(
            shared,
            window,
            cx,
            move |host| host.session_batch(&request),
            move |this, reply, _window, cx| {
                this.session_delete_prepare_landed(reply, cx);
            },
        );
    }

    /// Prepare 落地：按组序把每行的核验结果写回清单；硬错误（传输、
    /// 拒绝）整体转为 Failed，清单仍可关闭。
    fn session_delete_prepare_landed(
        &mut self,
        reply: Result<
            SessionBatchReply,
            lyrel_core::host::session_operations::SessionOperationError,
        >,
        cx: &mut Context<Self>,
    ) {
        let live = (self.workspace.live_sessions)(cx);
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let Some(flow) = settings.sessions.delete.as_mut() else {
            return;
        };
        match reply {
            Ok(SessionBatchReply::Prepare(result)) => {
                for (ix, group_result) in result.groups.iter().enumerate() {
                    for item in &group_result.items {
                        let row = flow
                            .rows
                            .iter_mut()
                            .find(|row| row.group == Some(ix) && row.identity.id == item.source_id);
                        let Some(row) = row else {
                            continue;
                        };
                        row.plan = Some(match item.status {
                            SessionBatchStatus::Eligible => SessionDeletePlan::Eligible {
                                delete_ids: item.delete_ids.clone(),
                            },
                            _ => {
                                let reason = item.reason.clone().unwrap_or(
                                    lyrel_core::host::session_operations::batch::SessionBatchReason::new(
                                        SessionBatchReasonKind::Provider,
                                        "unexpected prepare answer",
                                    ),
                                );
                                SessionDeletePlan::Ineligible {
                                    kind: reason.kind,
                                    message: reason.message.into(),
                                }
                            }
                        });
                    }
                }
                flow.exclude_open_sessions(&live);
                flow.phase = SessionDeletePhase::Ready;
            }
            Ok(_) => {
                flow.phase = SessionDeletePhase::Failed("unexpected batch reply".into());
            }
            Err(error) => {
                flow.phase = SessionDeletePhase::Failed(error.message.into());
            }
        }
        cx.notify();
    }

    fn set_session_delete_consent(&mut self, which: usize, checked: bool, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut()
            && let Some(flow) = settings.sessions.delete.as_mut()
        {
            match which {
                0 => flow.consent_irreversible = checked,
                _ => flow.consent_checked = checked,
            }
        }
        cx.notify();
    }

    /// 确认清单通过：Execute 只携带 prepare 判为 eligible 的行，每行
    /// 绑定它prepare 时算出的精确原生级联；分组与顺序和 prepare 完全一致。
    fn confirm_session_delete(&mut self, cx: &mut Context<Self>) {
        let live = (self.workspace.live_sessions)(cx);
        if let Some(flow) = self
            .active_settings_mut()
            .and_then(|settings| settings.sessions.delete.as_mut())
            && flow.can_confirm()
            && flow.exclude_open_sessions(&live)
        {
            flow.consent_irreversible = false;
            flow.consent_checked = false;
            cx.notify();
            return;
        }
        let Some(settings) = self.active_settings() else {
            return;
        };
        let host = settings.sessions.host;
        let Some(flow) = settings.sessions.delete.as_ref() else {
            return;
        };
        if !flow.can_confirm() {
            return;
        }
        let mut stores = Vec::new();
        let mut execute_groups: Vec<SessionBatchExecuteGroup> = Vec::new();
        for (ix, group) in flow.groups.iter().enumerate() {
            let confirmations: Vec<SessionBatchConfirmation> = flow
                .rows
                .iter()
                .filter(|row| row.group == Some(ix))
                .filter_map(|row| match &row.plan {
                    Some(SessionDeletePlan::Eligible { delete_ids }) => {
                        Some(SessionBatchConfirmation {
                            source_id: row.identity.id.clone(),
                            expected_delete_ids: delete_ids.clone(),
                        })
                    }
                    _ => None,
                })
                .collect();
            if !confirmations.is_empty() {
                let source = flow.rows.iter().find(|row| row.group == Some(ix)).unwrap();
                stores.push(session_history::store_group_key(&source.identity));
                execute_groups.push(SessionBatchExecuteGroup {
                    cwd: group.cwd.clone(),
                    launch_argv: group.launch_argv.clone(),
                    scope: group.scope.clone(),
                    confirmations,
                });
            }
        }
        let request = SessionBatchRequest::Execute(Box::new(SessionBatchExecute {
            expected_host: host.0,
            agent: CLIAgent::Hermes,
            groups: execute_groups,
        }));
        let owner = flow.host.clone();
        let same_host = HostRegistry::lookup(cx, host)
            .is_some_and(|current| std::sync::Arc::ptr_eq(&owner, &current));
        let Some(flow) = self
            .active_settings_mut()
            .and_then(|settings| settings.sessions.delete.as_mut())
        else {
            return;
        };
        if !same_host {
            flow.phase = SessionDeletePhase::Failed(t(L10nKey::PanelSessionsDisconnected).into());
            cx.notify();
            return;
        }
        flow.phase = SessionDeletePhase::Executing;
        cx.notify();
        HostOps::run(
            owner,
            cx,
            move |host| host.session_batch(&request),
            move |this, reply, cx| this.session_delete_execute_landed(host, stores, reply, cx),
        );
    }

    /// Execute 落地：整组结果进结果页；硬错误转 Failed，清单仍可关闭。
    fn session_delete_execute_landed(
        &mut self,
        host: HostId,
        stores: Vec<String>,
        reply: Result<
            SessionBatchReply,
            lyrel_core::host::session_operations::SessionOperationError,
        >,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let Some(flow) = settings.sessions.delete.as_mut() else {
            return;
        };
        match reply {
            Ok(SessionBatchReply::Execute(result)) => {
                let removed: Vec<_> = stores
                    .into_iter()
                    .zip(&result.groups)
                    .filter_map(|(store, group)| {
                        let ids: Vec<_> = group
                            .items
                            .iter()
                            .filter(|item| item.status == SessionBatchStatus::Deleted)
                            .flat_map(|item| item.delete_ids.iter().cloned())
                            .collect();
                        (!ids.is_empty()).then_some((store, ids))
                    })
                    .collect();
                flow.phase = SessionDeletePhase::Done(result);
                if !removed.is_empty() {
                    cx.emit(crate::ui::settings_window::SettingsEvent::SessionsDeleted {
                        host,
                        removed,
                    });
                }
            }
            Ok(_) => {
                flow.phase = SessionDeletePhase::Failed("unexpected batch reply".into());
            }
            Err(error) => {
                flow.phase = SessionDeletePhase::Failed(error.message.into());
            }
        }
        cx.notify();
    }

    /// 清掉选择与删除流并重新扫描设置页。
    fn finish_session_delete(&mut self, cx: &mut Context<Self>) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        if settings.sessions.delete.take().is_none() {
            return;
        };
        let state = &mut settings.sessions;
        state.selected.clear();
        state.reset_listing();
        cx.notify();
    }

    fn cancel_session_delete(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.delete = None;
        }
        cx.notify();
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

        let toolbar = self.sessions_toolbar(state, home.as_deref(), cx);
        let left = self.sessions_list(
            state,
            &rows,
            landed_listing.as_ref(),
            now,
            home.as_deref(),
            cx,
        );
        let side = self.sessions_side(state, &rows, host, home.as_deref(), cx);

        v_flex()
            .id("settings-sessions")
            .test_support()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(toolbar)
            .when(state.activity == SessionActivity::Custom, |page| {
                page.child(self.sessions_custom_range(state))
            })
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(v_flex().flex_1().min_w_0().min_h_0().child(left))
                    .child(
                        div()
                            .id("sessions-side")
                            .test_support()
                            .w(gpui_kit::rems(SIDE_W / 16.))
                            .max_w(gpui_kit::relative(0.44))
                            .flex_shrink_0()
                            .min_h_0()
                            .overflow_y_scroll()
                            .border_l_1()
                            .border_color(tk.k08)
                            .pl(px(16.))
                            .child(side),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_sessions_host(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let host = self.active_settings()?.sessions.host;
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
        Some(host_menu)
    }

    /// 顶部筛选行：标题/ID 搜索、智能体、最近活动、目录。
    fn sessions_toolbar(
        &self,
        state: &SessionsState,
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

        // 目录筛选只有两项：所有目录，或打开选择器挑一个 —— 不再列出扫
        // 出来的会话目录，也不带搜索。本机走系统文件夹选择器；SSH 主机
        // 开只读浏览器（见 directory_browser）。
        let dir_label: gpui_kit::SharedString = state
            .directory
            .as_ref()
            .map(|dir| crate::ui::home::display_path(dir, home).into())
            .unwrap_or_else(|| t(L10nKey::SessionsAllDirectories).into());
        let directory_entries = vec![
            MenuEntry::item(
                t(L10nKey::SessionsAllDirectories),
                state.directory.is_none(),
                move |this, _window, cx| {
                    this.select_sessions_directory(None, cx);
                },
            ),
            MenuEntry::item(
                t(L10nKey::PanelSessionsChooseDirectory),
                false,
                move |this, window, cx| {
                    this.choose_sessions_directory(window, cx);
                },
            ),
        ];
        let directory_menu = self.settings_dropdown(
            "sessions-directory",
            dir_label,
            lead(L10nKey::SessionsDirectoryLead),
            directory_entries,
            cx,
        );

        h_flex()
            .id("sessions-toolbar")
            .test_support()
            .w_full()
            .items_center()
            .flex_wrap()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(gpui_kit::rems(7.5))
                    .child(kit::search_field(&state.search, &tk)),
            )
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
        // 删除按钮显示「当前已知可删」的数量：无客户端阻挡原因的行。
        // 点击后先走所属主机的 prepare 重新核验，真正的删除只发生在
        // 确认清单之后。
        let candidates: Vec<SessionDeleteCandidate> = selected_rows
            .iter()
            .map(|row| SessionDeleteCandidate {
                identity: SessionIdentity::of(host, &row.session),
                title: row.session.title.clone().into(),
                blocked: row.blocked.clone(),
                cwd: row.session.cwd.clone(),
            })
            .collect();
        let deletable = candidates
            .iter()
            .filter(|candidate| candidate.blocked.is_empty())
            .count();
        let start = candidates.clone();
        let busy = state.delete.is_some();
        let delete = kit::button(
            "sessions-delete-selected",
            t_fmt(
                L10nKey::SessionsDeleteSelectedFmt,
                &[("count", &deletable.to_string())],
            ),
            kit::BtnKind::Danger,
        )
        .disabled(deletable == 0 || busy)
        .on_click(cx.listener(move |this, _, window, cx| {
            this.start_session_delete(window, start.clone(), cx);
        }));
        body.child(list)
            .child(h_flex().pt(px(4.)).gap(px(8.)).child(hide).child(delete))
            .into_any_element()
    }

    /// 删除确认／结果弹层。Preparing 显示重新核验进度；Ready 按
    /// prepare 的重新核验结果列出来源（存储组）、目标（标题）、影响
    /// 范围（确切原生记录）与被排除的逐条原因，同意清单两项都勾选
    /// 才能确认；Executing 只读；Done 聚合报告 deleted/skipped/failed
    /// 并逐条给出原因。执行中不可关闭 —— 原生操作已在飞行，结果必须
    /// 被看到。
    pub(crate) fn render_sessions_delete_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let settings = self.active_settings()?;
        let flow = settings.sessions.delete.as_ref()?;
        let tk = Tk::of(cx);
        let phase = &flow.phase;

        let mut body = v_flex().w_full().min_h_0().gap(px(10.));
        let mut footer = h_flex().w_full().items_center().gap(px(8.));

        match phase {
            SessionDeletePhase::Preparing => {
                body = body.child(
                    div()
                        .id("sessions-delete-status")
                        .test_support()
                        .text_size(fs(12.))
                        .text_color(tk.k45)
                        .child(t(L10nKey::SessionsDeletePreparing)),
                );
                footer = footer.child(
                    kit::button(
                        "sessions-delete-cancel",
                        t(L10nKey::SessionsCancelSelection),
                        kit::BtnKind::Link,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.cancel_session_delete(cx);
                    })),
                );
            }
            SessionDeletePhase::Ready => {
                let host = flow.host.id();
                let host_label = self
                    .agent_hooks_machines(cx)
                    .into_iter()
                    .find(|machine| machine.host == host)
                    .map_or_else(|| format!("{host:?}"), |machine| machine.label);
                body = body
                    .child(
                        div()
                            .text_size(fs(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} · {host_label}", t(L10nKey::SettingsHost))),
                    )
                    .child(
                        div()
                            .id("sessions-delete-status")
                            .test_support()
                            .text_size(fs(12.))
                            .text_color(tk.k45)
                            .child(t(L10nKey::SessionsDeleteConfirmScope)),
                    )
                    .child(self.sessions_delete_rows(cx))
                    .child(
                        v_flex()
                            .id("sessions-delete-consent")
                            .test_support()
                            .gap(px(6.))
                            .pt(px(2.))
                            .child(self.sessions_delete_consent_row(
                                0,
                                flow.consent_irreversible,
                                L10nKey::SessionsDeleteConsentIrreversible,
                                cx,
                            ))
                            .child(self.sessions_delete_consent_row(
                                1,
                                flow.consent_checked,
                                L10nKey::SessionsDeleteConsentStore,
                                cx,
                            )),
                    );
                let eligible = flow.eligible();
                let can_confirm = flow.can_confirm();
                footer = footer
                    .child(
                        div()
                            .flex_1()
                            .text_size(fs(11.5))
                            .text_color(tk.k45)
                            .child(t_fmt(
                                L10nKey::SessionsDeleteSelectedFmt,
                                &[("count", &eligible.to_string())],
                            )),
                    )
                    .child(
                        kit::button(
                            "sessions-delete-cancel",
                            t(L10nKey::SessionsCancelSelection),
                            kit::BtnKind::Link,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.cancel_session_delete(cx);
                        })),
                    )
                    .child(
                        kit::button(
                            "sessions-delete-confirm",
                            t_fmt(
                                L10nKey::SessionsDeleteConfirmFmt,
                                &[("count", &eligible.to_string())],
                            ),
                            kit::BtnKind::Danger,
                        )
                        .disabled(!can_confirm)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_session_delete(cx);
                        })),
                    );
            }
            SessionDeletePhase::Failed(message) => {
                body = body
                    .child(
                        div()
                            .id("sessions-delete-status")
                            .test_support()
                            .text_size(fs(12.))
                            .text_color(tk.danger)
                            .child(format!("{}: {message}", t(L10nKey::SessionsDeleteFailed))),
                    )
                    .child(self.sessions_delete_rows(cx));
                footer = footer.child(
                    kit::button(
                        "sessions-delete-cancel",
                        t(L10nKey::SessionsCancelSelection),
                        kit::BtnKind::Link,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.cancel_session_delete(cx);
                    })),
                );
            }
            SessionDeletePhase::Executing => {
                body = body.child(
                    div()
                        .id("sessions-delete-status")
                        .test_support()
                        .text_size(fs(12.))
                        .text_color(tk.k45)
                        .child(t(L10nKey::SessionsDeleteExecuting)),
                );
            }
            SessionDeletePhase::Done(result) => {
                let totals = result.totals();
                body = body
                    .child(
                        h_flex()
                            .id("sessions-delete-totals")
                            .test_support()
                            .items_center()
                            .gap(px(6.))
                            .child(div().size(px(6.)).rounded_full().bg(tk.fg))
                            .child(
                                div()
                                    .text_size(fs(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t_fmt(
                                        L10nKey::SessionsDeleteResultFmt,
                                        &[
                                            ("deleted", &totals.deleted.to_string()),
                                            ("skipped", &totals.skipped.to_string()),
                                            ("failed", &totals.failed.to_string()),
                                        ],
                                    )),
                            ),
                    )
                    .child(self.sessions_delete_rows(cx));
                footer = footer.child(div().flex_1()).child(
                    kit::button(
                        "sessions-delete-done",
                        t(L10nKey::SessionsDeleteDone),
                        kit::BtnKind::Primary,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.finish_session_delete(cx);
                    })),
                );
            }
        }

        let dismissable = !matches!(phase, SessionDeletePhase::Executing);
        let card = crate::ui::dialog::card(
            "sessions-delete-dialog",
            t(L10nKey::SessionsDeleteConfirmTitle),
            560.,
            cx,
        )
        .max_w_full()
        .max_h_full()
        .child(crate::ui::dialog::header(
            t(L10nKey::SessionsDeleteConfirmTitle),
            cx,
        ))
        .child(
            div()
                .id("sessions-delete-body")
                .test_support()
                .min_h_0()
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .child(body),
        )
        .child(crate::ui::dialog::footer(cx).child(footer));
        Some(
            div()
                .id("sessions-delete-overlay")
                .test_support()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .p_3()
                .bg(crate::ui::theme::overlay_background(cx))
                .when(dismissable, |overlay| {
                    overlay.on_key_down(cx.listener(
                        |this, event: &gpui_kit::KeyDownEvent, _window, cx| {
                            if event.keystroke.key == "escape" {
                                this.cancel_session_delete(cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                })
                .child(card)
                .into_any_element(),
        )
    }

    /// 弹层里的同意清单一行。
    fn sessions_delete_consent_row(
        &self,
        which: usize,
        checked: bool,
        label: L10nKey,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(format!("sessions-delete-consent-{which}"))
            .test_support()
            .items_center()
            .gap(px(8.))
            .child(
                Checkbox::new(format!("sessions-delete-consent-check-{which}"))
                    .checked(checked)
                    .small()
                    .on_click(cx.listener(move |this, on, _window, cx| {
                        this.set_session_delete_consent(which, *on, cx);
                    })),
            )
            .child(div().flex_1().min_w_0().text_size(fs(12.)).child(t(label)))
            .into_any_element()
    }

    /// 弹层里的逐条清单：被排除的行给客户端原因；prepare 后 eligible
    /// 的行显示存储位置与确切原生记录，ineligible 显示原生原因。
    /// Done 后显示每行的 deleted/skipped/failed。
    fn sessions_delete_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let Some(settings) = self.active_settings() else {
            return div().into_any_element();
        };
        let Some(flow) = settings.sessions.delete.as_ref() else {
            return div().into_any_element();
        };
        let outcomes: std::collections::HashMap<(usize, String), SessionBatchItem> =
            match &flow.phase {
                SessionDeletePhase::Done(result) => result
                    .groups
                    .iter()
                    .enumerate()
                    .flat_map(|(ix, group)| {
                        group
                            .items
                            .iter()
                            .map(move |item| ((ix, item.source_id.clone()), item.clone()))
                    })
                    .collect(),
                _ => Default::default(),
            };
        let mut list = v_flex()
            .id("sessions-delete-rows")
            .test_support()
            .w_full()
            .gap(px(2.));
        for (ix, row) in flow.rows.iter().enumerate() {
            let status = row
                .group
                .and_then(|group| outcomes.get(&(group, row.identity.id.clone())).cloned());
            let (note, color) = match &flow.phase {
                SessionDeletePhase::Done(_) => match &status {
                    Some(item) if item.status == SessionBatchStatus::Deleted => {
                        (t(L10nKey::SessionsDeleteDone).into(), tk.fg)
                    }
                    Some(item) => {
                        let kind = item
                            .reason
                            .as_ref()
                            .map(|reason| reason.kind)
                            .unwrap_or(SessionBatchReasonKind::Provider);
                        let message = item
                            .reason
                            .as_ref()
                            .map(|reason| reason.message.clone())
                            .unwrap_or_default();
                        (
                            format!("{} · {message}", session_batch_reason_label(kind)).into(),
                            if item.status == SessionBatchStatus::Failed {
                                tk.danger
                            } else {
                                tk.warn_text
                            },
                        )
                    }
                    None => ("".into(), tk.k45),
                },
                _ => match (&row.excluded, &row.plan) {
                    (Some(blocker), _) => (blocker.label(), tk.k45),
                    (None, Some(SessionDeletePlan::Eligible { delete_ids })) => (
                        t_fmt(
                            L10nKey::SessionsDeleteCascadeFmt,
                            &[("count", &delete_ids.len().to_string())],
                        )
                        .into(),
                        tk.k45,
                    ),
                    (None, Some(SessionDeletePlan::Ineligible { kind, message })) => (
                        format!("{} · {message}", session_batch_reason_label(*kind)).into(),
                        tk.warn_text,
                    ),
                    (None, None) => ("".into(), tk.k45),
                },
            };
            let store = row
                .group
                .map(|group| flow.groups[group].store_label.clone())
                .unwrap_or_default();
            let mut entry = v_flex()
                .id(format!("sessions-delete-row-{ix}"))
                .test_support()
                .w_full()
                .gap_1()
                .py(px(3.))
                .child(
                    h_flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(fs(12.))
                                        .text_color(tk.fg)
                                        .child(row.title.clone()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .whitespace_normal()
                                        .text_size(fs(11.))
                                        .text_color(tk.k45)
                                        .child(store),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .max_w(gpui_kit::relative(0.45))
                                .whitespace_normal()
                                .text_size(fs(11.))
                                .text_color(color)
                                .child(note),
                        ),
                );
            if let (
                SessionDeletePhase::Ready,
                None,
                Some(SessionDeletePlan::Eligible { delete_ids }),
            ) = (&flow.phase, &row.excluded, &row.plan)
            {
                for (target, id) in delete_ids.iter().enumerate() {
                    entry = entry.child(
                        div()
                            .id(format!("sessions-delete-target-{ix}-{target}"))
                            .aria_label(gpui_kit::SharedString::from(id.clone()))
                            .test_support()
                            .whitespace_normal()
                            .pl_2()
                            .text_size(fs(11.))
                            .text_color(tk.fg)
                            .child(id.clone()),
                    );
                }
            }
            list = list.child(entry);
        }
        list.into_any_element()
    }
}

/// 批量删除里原生原因的一行话；与选中清单的客户端阻挡共用措辞。
fn session_batch_reason_label(kind: SessionBatchReasonKind) -> gpui_kit::SharedString {
    let key = match kind {
        SessionBatchReasonKind::Active => L10nKey::SessionsDeleteReasonActive,
        SessionBatchReasonKind::Stale => L10nKey::SessionsDeleteReasonStale,
        SessionBatchReasonKind::UnsupportedProvider => L10nKey::SessionsBlockerSourceUnsupported,
        SessionBatchReasonKind::UnconfirmedStore => L10nKey::SessionsBlockerStoreUnconfirmed,
        SessionBatchReasonKind::Provider => L10nKey::SessionsDeleteReasonProvider,
    };
    t(key).into()
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
