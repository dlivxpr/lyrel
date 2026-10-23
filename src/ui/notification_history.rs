//! 应用收到的 Toast 历史；本机共享，与 daemon 和系统通知中心无关。
//!
//! 内存修改在 GPUI 事件循环串行执行；一个 writer 按相同顺序处理操作并原子写盘。
//! 延迟补齐来源只更新仍存在的记录，绝不会把 clear/retention 删除的记录加回来。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;

use gpui_kit::component::notification::{Notification as Toast, NotificationType};
use gpui_kit::{App, AppContext as _, ClickEvent, Context, ElementId, Entity, Global, SharedString, Window};
use serde::{Deserialize, Serialize};

use crate::core::session::{WorkspaceId, WorkspaceStore};
use crate::ui::app::LyrelApp;
use crate::ui::host_registry::HostId;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::pane::PaneSlot;

mod inbox_view;
pub(crate) use inbox_view::NotificationBell;
#[cfg(test)]
mod tests;

const RETENTION: usize = 500;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum Severity { Success, Info, Warning, Error }

impl Severity {
    pub(crate) fn label(self) -> &'static str {
        t(match self {
            Self::Success => L10nKey::NotificationSuccess,
            Self::Info => L10nKey::NotificationInfo,
            Self::Warning => L10nKey::NotificationWarning,
            Self::Error => L10nKey::NotificationError,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Source {
    workspace: WorkspaceId,
    machine_workspace: WorkspaceId,
    host: u64,
    tab: Option<lyrel_core::core::machine::TabId>,
    pane: Option<u64>,
    label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    id: uuid::Uuid,
    timestamp: i64,
    severity: Severity,
    message: String,
    source: Option<Source>,
    unread: bool,
}

impl Record {
    pub(crate) fn id(&self) -> uuid::Uuid { self.id }
    pub(crate) fn timestamp(&self) -> i64 { self.timestamp }
    pub(crate) fn severity(&self) -> Severity { self.severity }
    pub(crate) fn message(&self) -> &str { &self.message }
    pub(crate) fn source(&self) -> Option<&Source> { self.source.as_ref() }
    pub(crate) fn is_unread(&self) -> bool { self.unread }
}

/// 应用通知值同时持有可持久化内容和原有 Toast（含关闭/重试行为）。
/// Kit 的 Toast 不公开内容读取 API，因此在构造时保留历史，而非读取私有字段。
pub(crate) struct Notification {
    toast: Toast,
    severity: Severity,
    title: SharedString,
    message: SharedString,
    source: Option<Source>,
    origin_pane: Option<(WorkspaceId, u64)>,
}

impl Notification {
    pub(crate) fn new() -> Self {
        Self { toast: Toast::new(), severity: Severity::Info, title: "".into(), message: "".into(), source: None, origin_pane: None }
    }
    pub(crate) fn info(message: impl Into<SharedString>) -> Self { Self::new().message(message) }
    pub(crate) fn success(message: impl Into<SharedString>) -> Self { Self::new().message(message).with_type(NotificationType::Success) }
    pub(crate) fn warning(message: impl Into<SharedString>) -> Self { Self::new().message(message).with_type(NotificationType::Warning) }
    pub(crate) fn error(message: impl Into<SharedString>) -> Self { Self::new().message(message).with_type(NotificationType::Error) }
    pub(crate) fn message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = message.into();
        self.toast = self.toast.message(self.message.clone());
        self
    }
    pub(crate) fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self.toast = self.toast.title(self.title.clone());
        self
    }
    pub(crate) fn with_type(mut self, kind: NotificationType) -> Self {
        self.severity = match kind {
            NotificationType::Info => Severity::Info,
            NotificationType::Success => Severity::Success,
            NotificationType::Warning => Severity::Warning,
            NotificationType::Error => Severity::Error,
        };
        self.toast = self.toast.with_type(kind);
        self
    }
    pub(crate) fn id<T: 'static>(mut self) -> Self { self.toast = self.toast.id::<T>(); self }
    pub(crate) fn id1<T: 'static>(mut self, key: impl Into<ElementId>) -> Self { self.toast = self.toast.id1::<T>(key); self }
    pub(crate) fn autohide(mut self, value: bool) -> Self { self.toast = self.toast.autohide(value); self }
    pub(crate) fn on_click(mut self, callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.toast = self.toast.on_click(callback);
        self
    }
    pub(crate) fn source(mut self, source: Source) -> Self { self.source = Some(source); self }
    pub(crate) fn pane(mut self, workspace: Option<WorkspaceId>, pane: u64) -> Self {
        self.origin_pane = workspace.map(|workspace| (workspace, pane));
        self
    }
}

impl From<String> for Notification { fn from(value: String) -> Self { Self::info(value) } }
impl From<&str> for Notification { fn from(value: &str) -> Self { Self::info(value.to_owned()) } }
impl From<SharedString> for Notification { fn from(value: SharedString) -> Self { Self::info(value) } }

pub(crate) trait NotificationWindowExt {
    fn notify_app(&mut self, notification: impl Into<Notification>, cx: &mut App);
}

impl NotificationWindowExt for Window {
    fn notify_app(&mut self, notification: impl Into<Notification>, cx: &mut App) {
        ensure(cx);
        let notification = notification.into();
        let message = match (notification.title.is_empty(), notification.message.is_empty()) {
            (true, _) => notification.message.to_string(),
            (_, true) => notification.title.to_string(),
            _ => format!("{} — {}", notification.title, notification.message),
        };
        let id = uuid::Uuid::new_v4();
        let needs_source = notification.source.is_none();
        let origin_pane = notification.origin_pane;
        history(cx).clone().update(cx, |inbox, cx| {
            inbox.append(Record { id, timestamp: chrono::Utc::now().timestamp(), severity: notification.severity, message, source: notification.source, unread: true });
            cx.notify();
        });
        // Callers often hold the LyrelApp lease. Resolve its source only after
        // that lease is returned; persist it as an update, never as an append.
        if needs_source {
            self.defer(cx, move |window, cx| {
                let app = source_app(window, cx);
                let app = match origin_pane {
                    Some((workspace, _)) if app.as_ref().is_none_or(|app| app.read(cx).workspace != workspace) => {
                        if cx.has_global::<crate::ui::windows::WindowRegistry>() {
                            crate::ui::windows::WindowRegistry::app_for(cx, workspace).and_then(|app| app.upgrade())
                        } else { None }
                    }
                    _ => app,
                };
                let Some(app) = app else { return; };
                let source = if origin_pane.is_none() && root_app(window, cx).is_none() {
                    app.read(cx).notification_workspace_source(cx)
                } else {
                    app.read(cx).notification_source_for_pane(origin_pane.map(|(_, pane)| pane), window, cx)
                };
                history(cx).clone().update(cx, |inbox, cx| {
                    inbox.attach_source(id, source);
                    cx.notify();
                });
            });
        }
        gpui_kit::component::WindowExt::push_notification(self, notification.toast, cx);
    }
}

struct InboxGlobal(Entity<Inbox>);
impl Global for InboxGlobal {}

pub(crate) fn history(cx: &App) -> &Entity<Inbox> { &cx.global::<InboxGlobal>().0 }

fn ensure(cx: &mut App) {
    if cx.has_global::<InboxGlobal>() { return; }
    #[cfg(test)]
    crate::core::config::pin_test_config_dir();
    let dir = crate::core::config::config_dir_path().unwrap_or_else(std::env::temp_dir);
    #[cfg(not(test))]
    let path = dir.join("notifications.json");
    #[cfg(test)]
    let path = dir.join(format!("notifications-test-{}.json", uuid::Uuid::new_v4()));
    install(cx, path);
}

fn install(cx: &mut App, path: PathBuf) {
    let inbox = Inbox::open(path);
    if let Some(global) = cx.try_global::<InboxGlobal>() {
        let entity = global.0.clone();
        entity.update(cx, |current, cx| { *current = inbox; cx.notify(); });
    } else {
        let entity = cx.new(|_| inbox);
        cx.set_global(InboxGlobal(entity));
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Snapshot { records: VecDeque<Record> }

impl Snapshot {
    fn append(&mut self, record: Record) {
        self.records.push_back(record);
        self.trim();
    }
    fn trim(&mut self) {
        while self.records.len() > RETENTION { self.records.pop_front(); }
    }
    fn apply(&mut self, operation: Operation) {
        match operation {
            Operation::Append(record) => self.append(record),
            Operation::Clear => self.records.clear(),
            Operation::Read => self.records.iter_mut().for_each(|record| record.unread = false),
            Operation::Source(id, source) => {
                if let Some(record) = self.records.iter_mut().find(|record| record.id == id) { record.source = Some(source); }
            }
        }
    }
}


enum Operation { Append(Record), Clear, Read, Source(uuid::Uuid, Source) }
enum Write { Apply(Operation), #[cfg(test)] Flush(mpsc::Sender<std::io::Result<()>>) }

pub(crate) struct Inbox {
    snapshot: Snapshot,
    writer: Option<mpsc::Sender<Write>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Inbox {
    fn open(path: PathBuf) -> Self {
        let mut snapshot: Snapshot = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                log::error!("cannot read notification history {}: {error}", path.display());
                Snapshot::default()
            }),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound { log::error!("cannot read notification history {}: {error}", path.display()); }
                Snapshot::default()
            }
        };
        snapshot.trim();
        let persisted = snapshot.clone();
        let (writer, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new().name("notification-history".into()).spawn(move || {
            if let Some(parent) = path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    log::error!("cannot create notification storage {}: {error}", parent.display());
                }
            }
            let mut persisted = persisted;
            while let Ok(write) = receiver.recv() {
                let mut pending = Some(write);
                let mut dirty = false;
                while let Some(write) = pending {
                    match write {
                        Write::Apply(operation) => { persisted.apply(operation); dirty = true; }
                        #[cfg(test)]
                        Write::Flush(done) => {
                            let _ = done.send(save(&path, &persisted));
                            dirty = false;
                        }
                    }
                    pending = receiver.try_recv().ok();
                }
                if dirty {
                    if let Err(error) = save(&path, &persisted) {
                        log::error!("cannot persist notification history {}: {error}", path.display());
                    }
                }
            }
        }).expect("start notification history writer");
        Self { snapshot, writer: Some(writer), worker: Some(worker) }
    }
    pub(crate) fn records(&self) -> impl DoubleEndedIterator<Item = &Record> + ExactSizeIterator { self.snapshot.records.iter() }
    pub(crate) fn unread_count(&self) -> usize { self.records().filter(|record| record.is_unread()).count() }
    fn send(&self, operation: Operation) {
        if self.writer.as_ref().is_some_and(|writer| writer.send(Write::Apply(operation)).is_err()) { log::error!("notification history writer stopped"); }
    }
    fn append(&mut self, record: Record) { self.send(Operation::Append(record.clone())); self.snapshot.append(record); }
    fn attach_source(&mut self, id: uuid::Uuid, source: Source) {
        if let Some(record) = self.snapshot.records.iter_mut().find(|record| record.id == id) {
            record.source = Some(source.clone());
            self.send(Operation::Source(id, source));
        }
    }
    fn clear(&mut self) { self.snapshot.records.clear(); self.send(Operation::Clear); }
    fn mark_read(&mut self) {
        if self.unread_count() == 0 { return; }
        self.snapshot.records.iter_mut().for_each(|record| record.unread = false);
        self.send(Operation::Read);
    }
    #[cfg(test)]
    fn flush(&self) {
        let (done, pending) = mpsc::channel();
        self.writer.as_ref().unwrap().send(Write::Flush(done)).unwrap();
        pending.recv().unwrap().unwrap();
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // Closing the GUI drains queued operations, rather than abandoning an
        // in-flight clear or the last received event at process shutdown.
        self.writer.take();
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

fn save(path: &std::path::Path, snapshot: &Snapshot) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    lyrel_core::core::config::write_atomic_private(path, &bytes)
}

fn root_app(window: &Window, cx: &App) -> Option<Entity<LyrelApp>> {
    window.root::<gpui_kit::component::Root>()??.read(cx).view().clone().downcast::<LyrelApp>().ok()
}

fn source_app(window: &Window, cx: &App) -> Option<Entity<LyrelApp>> {
    let view = window.root::<gpui_kit::component::Root>()??.read(cx).view().clone();
    if let Ok(app) = view.clone().downcast::<LyrelApp>() { return Some(app); }
    view.downcast::<crate::ui::settings_window::SettingsWindow>().ok()?.read(cx).app.upgrade()
}

impl LyrelApp {
    pub(crate) fn notification_source(&self, window: &Window, cx: &App) -> Source {
        self.notification_source_for_pane(None, window, cx)
    }

    fn notification_source_for_pane(&self, origin: Option<u64>, window: &Window, cx: &App) -> Source {
        let tab = match origin {
            Some(pane) => self.tabs.iter().find(|tab| tab.pane.leaves().iter().any(|slot| match slot {
                PaneSlot::Ready(view) => view.read(cx).pane_id == pane,
                PaneSlot::Connecting(view) => view.read(cx).spawn.restore_pane == Some(pane),
            })),
            None => self.tabs.get(self.active),
        };
        let pane = origin.or_else(|| tab.and_then(|tab| tab.pane.focused_or_first(window, cx)).map(|pane| pane.read(cx).pane_id));
        let mut source = self.notification_workspace_source(cx);
        if let Some(tab) = tab {
            let title = tab.leaf_title(Some(window), cx);
            if !title.is_empty() { source.label.push_str(" · "); source.label.push_str(&title); }
        }
        if let Some(pane) = pane { source.label.push_str(&format!(" · #{pane}")); }
        source.tab = tab.map(|tab| tab.tree_id.get());
        source.pane = pane;
        source
    }

    fn notification_workspace_source(&self, cx: &App) -> Source {
        let workspace = crate::ui::machine_mirror::display_name_for(cx, self.workspace).unwrap_or_else(|| self.workspace.to_string());
        Source {
            workspace: self.workspace,
            machine_workspace: WorkspaceStore::remote_ref(cx, self.workspace).map(|remote| remote.workspace).unwrap_or(self.workspace),
            host: WorkspaceStore::host_of(cx, self.workspace).0,
            tab: None,
            pane: None,
            label: format!("{} · {workspace}", self.remote_machine_label(cx)),
        }
    }

    fn notification_target(&self, source: &Source, cx: &App) -> Option<(usize, Option<PaneSlot>)> {
        if self.workspace != source.workspace || WorkspaceStore::host_of(cx, self.workspace).0 != source.host { return None; }
        if let Some(want) = source.tab {
            let (index, tab) = self.tabs.iter().enumerate().find(|(_, tab)| tab.tree_id.get() == want)?;
            let pane = match source.pane {
                Some(id) => Some(tab.pane.leaves().into_iter().find(|pane| match pane {
                    PaneSlot::Ready(view) => view.read(cx).pane_id == id,
                    PaneSlot::Connecting(view) => view.read(cx).spawn.restore_pane == Some(id),
                })?),
                None => None,
            };
            Some((index, pane))
        } else if let Some(id) = source.pane {
            self.tabs.iter().enumerate().find_map(|(index, tab)| {
                tab.pane.leaves().into_iter().find(|slot| match slot {
                    PaneSlot::Ready(view) => view.read(cx).pane_id == id,
                    PaneSlot::Connecting(view) => view.read(cx).spawn.restore_pane == Some(id),
                }).map(|slot| (index, Some(slot)))
            })
        } else { Some((self.active, None)) }
    }

    pub(crate) fn claim_notification_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.pending_notification_source.take() else { return; };
        if let Some((index, pane)) = self.notification_target(&source, cx) {
            self.active = index;
            if let Some(pane) = pane { self.focus_pane_command_target(pane.entity_id(), window, cx); }
            cx.notify();
        } else {
            self.pending_notification_source = Some(source);
        }
    }
}

fn open_app(source: &Source, window: &Window, cx: &App) -> Option<Entity<LyrelApp>> {
    if let Some(app) = root_app(window, cx).filter(|app| app.read(cx).workspace == source.workspace) { return Some(app); }
    if !cx.has_global::<crate::ui::windows::WindowRegistry>() { return None; }
    crate::ui::windows::WindowRegistry::app_for(cx, source.workspace)?.upgrade()
}

fn stored_source_exists(source: &Source, cx: &App) -> bool {
    let Some(machine) = crate::ui::machine_mirror::MachineMirrors::machine(cx, HostId(source.host)) else { return false; };
    let Some(workspace) = machine.workspaces.iter().find(|workspace| workspace.id == source.machine_workspace) else { return false; };
    if source.tab.is_none() && source.pane.is_none() { return true; }
    workspace.tabs.iter().any(|tab| source.tab.is_none_or(|want| tab.id == want) && source.pane.is_none_or(|pane| tab.root.pane_ids().contains(&pane)))
}

fn source_exists(source: &Source, window: &Window, cx: &App) -> bool {
    if let Some(app) = open_app(source, window, cx) { return app.read(cx).notification_target(source, cx).is_some(); }
    stored_source_exists(source, cx)
}

fn navigate(source: &Source, window: &mut Window, cx: &mut App) -> bool {
    if !source_exists(source, window, cx) { return false; }
    if open_app(source, window, cx).is_none() {
        if let Some(tab) = source.tab { crate::ui::windows::open_at_tab(cx, source.workspace, tab); }
        else { crate::ui::windows::open(cx, Some(source.workspace)); }
    }
    let Some(app) = open_app(source, window, cx) else { return false; };
    let source = source.clone();
    let workspace = source.workspace;
    let focus = move |window: &mut Window, cx: &mut App| {
        window.activate_window();
        app.update(cx, |app, cx| {
            app.pending_notification_source = Some(source);
            app.claim_notification_source(window, cx);
            cx.notify();
        });
    };
    if root_app(window, cx).is_some_and(|app| app.read(cx).workspace == workspace) { focus(window, cx); }
    else if let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, workspace) { let _ = handle.update(cx, |_, window, cx| focus(window, cx)); }
    true
}
