//! 应用收到的 Toast 历史；本机共享，与 daemon 和系统通知中心无关。
//!
//! 内存修改在 GPUI 事件循环串行执行；一个 writer 按相同顺序处理操作并原子写盘。
//! 延迟补齐来源只更新仍存在的记录，绝不会把 clear/retention 删除的记录加回来。
//!
//! 本模块只持有可持久化内容和通用 Toast 包装；「来源」的解析在
//! `NotificationSourceResolver` 后面：工作台在安装时提供查询，本 crate 不
//! 认识窗口注册表、机器镜像或应用实体。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;

use gpui_kit::component::notification::{Notification as Toast, NotificationType};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, ElementId, Entity, Global, SharedString, Window,
};
use lyrel_core::core::pane_sidecar::{PaneOwner, ScopedPane};
use serde::{Deserialize, Serialize};

use crate::session::WorkspaceId;

const RETENTION: usize = 500;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Severity {
    Success,
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        crate::i18n::t(match self {
            Self::Success => crate::i18n::L10nKey::NotificationSuccess,
            Self::Info => crate::i18n::L10nKey::NotificationInfo,
            Self::Warning => crate::i18n::L10nKey::NotificationWarning,
            Self::Error => crate::i18n::L10nKey::NotificationError,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub workspace: WorkspaceId,
    pub machine_workspace: WorkspaceId,
    pub host: u64,
    pub tab: Option<lyrel_core::core::machine::TabId>,
    pub pane: Option<u64>,
    #[serde(default)]
    pub pane_owner: Option<PaneOwner>,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: uuid::Uuid,
    pub timestamp: i64,
    pub severity: Severity,
    pub message: String,
    pub source: Option<Source>,
    pub unread: bool,
}

impl Record {
    pub fn id(&self) -> uuid::Uuid {
        self.id
    }
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }
    pub fn severity(&self) -> Severity {
        self.severity
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }
    pub fn is_unread(&self) -> bool {
        self.unread
    }
}

/// 应用通知值同时持有可持久化内容和原有 Toast（含关闭/重试行为）。
/// Kit 的 Toast 不公开内容读取 API，因此在构造时保留历史，而非读取私有字段。
pub struct Notification {
    toast: Toast,
    severity: Severity,
    title: SharedString,
    message: SharedString,
    source: Option<Source>,
    origin_pane: Option<(WorkspaceId, ScopedPane)>,
}

impl Notification {
    pub fn new() -> Self {
        Self {
            toast: Toast::new(),
            severity: Severity::Info,
            title: "".into(),
            message: "".into(),
            source: None,
            origin_pane: None,
        }
    }
    pub fn info(message: impl Into<SharedString>) -> Self {
        Self::new().message(message)
    }
    #[cfg(any(test, feature = "test-guards"))]
    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_type(NotificationType::Success)
    }
    pub fn warning(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_type(NotificationType::Warning)
    }
    pub fn error(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_type(NotificationType::Error)
    }
    pub fn message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = message.into();
        self.toast = self.toast.message(self.message.clone());
        self
    }
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self.toast = self.toast.title(self.title.clone());
        self
    }
    pub fn with_type(mut self, kind: NotificationType) -> Self {
        self.severity = match kind {
            NotificationType::Info => Severity::Info,
            NotificationType::Success => Severity::Success,
            NotificationType::Warning => Severity::Warning,
            NotificationType::Error => Severity::Error,
        };
        self.toast = self.toast.with_type(kind);
        self
    }
    pub fn id<T: 'static>(mut self) -> Self {
        self.toast = self.toast.id::<T>();
        self
    }
    pub fn id1<T: 'static>(mut self, key: impl Into<ElementId>) -> Self {
        self.toast = self.toast.id1::<T>(key);
        self
    }
    pub fn autohide(mut self, value: bool) -> Self {
        self.toast = self.toast.autohide(value);
        self
    }
    #[cfg(any(test, feature = "test-guards"))]
    pub fn placement(mut self, anchor: gpui_kit::Anchor) -> Self {
        self.toast = self.toast.placement(anchor);
        self
    }
    pub fn on_click(
        mut self,
        callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.toast = self.toast.on_click(callback);
        self
    }
    pub fn source(mut self, source: Source) -> Self {
        self.source = Some(source);
        self
    }
    pub fn pane(
        mut self,
        workspace: Option<WorkspaceId>,
        pane: u64,
        route: &crate::route::PaneRoute,
    ) -> Self {
        let pane = if route.is_local() {
            ScopedPane::client_local(pane)
        } else {
            ScopedPane::workspace(pane)
        };
        self.origin_pane = workspace.map(|workspace| (workspace, pane));
        self
    }
}

impl Default for Notification {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for Notification {
    fn from(value: String) -> Self {
        Self::info(value)
    }
}
impl From<&str> for Notification {
    fn from(value: &str) -> Self {
        Self::info(value.to_owned())
    }
}
impl From<SharedString> for Notification {
    fn from(value: SharedString) -> Self {
        Self::info(value)
    }
}

/// The shell-installed source resolver. A record that arrives without an
/// explicit source gets one after the toast is pushed — the delay keeps the
/// app lease out of the caller's hands — and the work of asking the window
/// registry, the machine mirrors and the app entity for the workspace/pane
/// identity stays above this crate, behind this one typed callback.
pub struct NotificationSourceResolver {
    resolve: Box<
        dyn Fn(Option<(WorkspaceId, ScopedPane)>, &mut Window, &mut App) -> Option<Source>,
    >,
}

impl NotificationSourceResolver {
    pub fn new(
        resolve: impl Fn(Option<(WorkspaceId, ScopedPane)>, &mut Window, &mut App) -> Option<Source>
        + 'static,
    ) -> Self {
        Self {
            resolve: Box::new(resolve),
        }
    }
}

impl Global for NotificationSourceResolver {}

pub fn install_notification_source_resolver(cx: &mut App, resolver: NotificationSourceResolver) {
    cx.set_global(resolver);
}

pub trait NotificationWindowExt {
    fn notify_app(&mut self, notification: impl Into<Notification>, cx: &mut App);
}

impl NotificationWindowExt for Window {
    fn notify_app(&mut self, notification: impl Into<Notification>, cx: &mut App) {
        ensure(cx);
        let notification = notification.into();
        let message = match (
            notification.title.is_empty(),
            notification.message.is_empty(),
        ) {
            (true, _) => notification.message.to_string(),
            (_, true) => notification.title.to_string(),
            _ => format!("{} — {}", notification.title, notification.message),
        };
        let id = uuid::Uuid::new_v4();
        let needs_source = notification.source.is_none();
        let origin_pane = notification.origin_pane;
        history(cx).clone().update(cx, |inbox, cx| {
            inbox.append(Record {
                id,
                timestamp: chrono::Utc::now().timestamp(),
                severity: notification.severity,
                message,
                source: notification.source,
                unread: true,
            });
            cx.notify();
        });
        // Callers often hold the app lease. Resolve its source only after
        // that lease is returned; persist it as an update, never an append.
        if needs_source {
            self.defer(cx, move |window, cx| {
                let Some(resolver) = cx.try_global::<NotificationSourceResolver>() else {
                    return;
                };
                let Some(source) = (resolver.resolve)(origin_pane, window, cx) else {
                    return;
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

pub fn history(cx: &App) -> &Entity<Inbox> {
    &cx.global::<InboxGlobal>().0
}

pub fn ensure(cx: &mut App) {
    if cx.has_global::<InboxGlobal>() {
        return;
    }
    #[cfg(any(test, feature = "test-guards"))]
    crate::config::pin_test_config_dir();
    let dir =
        lyrel_core::core::config::config_dir_path().unwrap_or_else(std::env::temp_dir);
    #[cfg(not(any(test, feature = "test-guards")))]
    let path = dir.join("notifications.json");
    #[cfg(any(test, feature = "test-guards"))]
    let path = dir.join(format!("notifications-test-{}.json", uuid::Uuid::new_v4()));
    install(cx, path);
}

pub fn install(cx: &mut App, path: PathBuf) {
    let inbox = Inbox::open(path);
    if let Some(global) = cx.try_global::<InboxGlobal>() {
        let entity = global.0.clone();
        entity.update(cx, |current, cx| {
            *current = inbox;
            cx.notify();
        });
    } else {
        let entity = cx.new(|_| inbox);
        cx.set_global(InboxGlobal(entity));
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Snapshot {
    records: VecDeque<Record>,
}

impl Snapshot {
    fn append(&mut self, record: Record) {
        self.records.push_back(record);
        self.trim();
    }
    fn trim(&mut self) {
        while self.records.len() > RETENTION {
            self.records.pop_front();
        }
    }
    fn apply(&mut self, operation: Operation) {
        match operation {
            Operation::Append(record) => self.append(record),
            Operation::Clear => self.records.clear(),
            Operation::Read => self
                .records
                .iter_mut()
                .for_each(|record| record.unread = false),
            Operation::Source(id, source) => {
                if let Some(record) = self.records.iter_mut().find(|record| record.id == id) {
                    record.source = Some(source);
                }
            }
        }
    }
}

enum Operation {
    Append(Record),
    Clear,
    Read,
    Source(uuid::Uuid, Source),
}
enum Write {
    Apply(Operation),
    #[cfg(feature = "test-guards")]
    Flush(mpsc::Sender<std::io::Result<()>>),
}

pub struct Inbox {
    snapshot: Snapshot,
    writer: Option<mpsc::Sender<Write>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Inbox {
    fn open(path: PathBuf) -> Self {
        let mut snapshot: Snapshot = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                log::error!(
                    "cannot read notification history {}: {error}",
                    path.display()
                );
                Snapshot::default()
            }),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::error!(
                        "cannot read notification history {}: {error}",
                        path.display()
                    );
                }
                Snapshot::default()
            }
        };
        snapshot.trim();
        let persisted = snapshot.clone();
        let (writer, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("notification-history".into())
            .spawn(move || {
                if let Some(parent) = path.parent() {
                    if let Err(error) = std::fs::create_dir_all(parent) {
                        log::error!(
                            "cannot create notification storage {}: {error}",
                            parent.display()
                        );
                    }
                }
                let mut persisted = persisted;
                while let Ok(write) = receiver.recv() {
                    let mut pending = Some(write);
                    let mut dirty = false;
                    while let Some(write) = pending {
                        match write {
                            Write::Apply(operation) => {
                                persisted.apply(operation);
                                dirty = true;
                            }
                            #[cfg(feature = "test-guards")]
                            Write::Flush(done) => {
                                let _ = done.send(save(&path, &persisted));
                                dirty = false;
                            }
                        }
                        pending = receiver.try_recv().ok();
                    }
                    if dirty {
                        if let Err(error) = save(&path, &persisted) {
                            log::error!(
                                "cannot persist notification history {}: {error}",
                                path.display()
                            );
                        }
                    }
                }
            })
            .expect("start notification history writer");
        Self {
            snapshot,
            writer: Some(writer),
            worker: Some(worker),
        }
    }
    pub fn records(&self) -> impl DoubleEndedIterator<Item = &Record> + ExactSizeIterator {
        self.snapshot.records.iter()
    }
    pub fn unread_count(&self) -> usize {
        self.records().filter(|record| record.is_unread()).count()
    }
    fn send(&self, operation: Operation) {
        if self
            .writer
            .as_ref()
            .is_some_and(|writer| writer.send(Write::Apply(operation)).is_err())
        {
            log::error!("notification history writer stopped");
        }
    }
    pub fn append(&mut self, record: Record) {
        self.send(Operation::Append(record.clone()));
        self.snapshot.append(record);
    }
    fn attach_source(&mut self, id: uuid::Uuid, source: Source) {
        if let Some(record) = self
            .snapshot
            .records
            .iter_mut()
            .find(|record| record.id == id)
        {
            record.source = Some(source.clone());
            self.send(Operation::Source(id, source));
        }
    }
    pub fn clear(&mut self) {
        self.snapshot.records.clear();
        self.send(Operation::Clear);
    }
    pub fn mark_read(&mut self) {
        if self.unread_count() == 0 {
            return;
        }
        self.snapshot
            .records
            .iter_mut()
            .for_each(|record| record.unread = false);
        self.send(Operation::Read);
    }
    #[cfg(feature = "test-guards")]
    pub fn flush(&self) {
        let (done, pending) = mpsc::channel();
        self.writer
            .as_ref()
            .unwrap()
            .send(Write::Flush(done))
            .unwrap();
        pending.recv().unwrap().unwrap();
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // Closing the GUI drains queued operations, rather than abandoning an
        // in-flight clear or the last received event at process shutdown.
        self.writer.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn save(path: &std::path::Path, snapshot: &Snapshot) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    lyrel_core::core::config::write_atomic_private(path, &bytes)
}
