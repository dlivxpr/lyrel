//! 通知历史的 shell 一侧：通用存储在 `lyrel-ui-base`，这里保留「来源」的
//! 解析与激活 —— 它要查询窗口注册表、机器镜像和应用实体，无法下放到基础库。
//! `notify_app` 延迟补来源时通过 `NotificationSourceResolver` 回调这里的查询。

use gpui_kit::{App, Entity, Window};

use crate::ui::app::LyrelApp;
use crate::ui::pane::PaneSlot;
use lyrel_ui_base::host_registry::HostId;
use lyrel_ui_base::session::WorkspaceStore;

mod inbox_view;
pub(crate) use inbox_view::{NotificationBell, NotificationBellEvent};
#[cfg(test)]
mod tests;

// The bounded inbox itself is foundation: re-exported for every consumer that
// pushes toasts or reads the bell.
#[cfg(test)]
pub(crate) use lyrel_ui_base::notification_history::install;
pub(crate) use lyrel_ui_base::notification_history::{
    Inbox, Notification, NotificationWindowExt, Record, Severity, Source, ensure, history,
};
pub use lyrel_ui_base::notification_history::{
    NotificationSourceResolver, install_notification_source_resolver,
};

/// Why a pane needs the user right now. Both states are live observations —
/// an agent's own wait or a program's OSC 7501 `blocked` report — and both
/// resolve out of the projection the moment the pane reports them settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingState {
    /// An agent session reported `Waiting`.
    Waiting,
    /// A program record reported `blocked`, with its reason when given.
    Blocked(Option<lyrel_core::core::program_status::BlockedKind>),
}

/// One pane that needs the user right now, projected from live observation.
/// Pending is never persisted: stale history can never assert a current
/// blocker, and a restart starts with an empty Pending list.
pub(crate) struct PendingItem {
    pub(crate) source: Source,
    pub(crate) state: PendingState,
    pub(crate) since_ms: Option<u64>,
}

/// The live Pending rows across every open window: every pane whose agent is
/// currently Waiting or whose program records report `blocked`. Records stay
/// in the inbox; this is the other half of "unresolved attention", read fresh
/// on each render.
pub(crate) fn pending(cx: &App) -> Vec<PendingItem> {
    let mut items = Vec::new();
    for app in crate::ui::windows::WindowRegistry::live_apps(cx) {
        app.read(cx).collect_pending(&mut items, cx);
    }
    items
}

/// Stable element identity for a Pending row: the Host, tab, pane and owner
/// it navigates to — never the row's list position. The Host scopes the id
/// because tab and pane numbers are only unique within one Host.
pub(crate) fn pending_element_id(source: &Source) -> String {
    let owner = match source.pane_owner {
        Some(lyrel_core::core::pane_sidecar::PaneOwner::Workspace) => "workspace",
        Some(lyrel_core::core::pane_sidecar::PaneOwner::ClientLocal) => "local",
        None => "unknown",
    };
    format!(
        "notification-pending-{}-{}-{}-{owner}",
        source.host,
        source.tab.map(|tab| tab.to_string()).unwrap_or_default(),
        source.pane.map(|pane| pane.to_string()).unwrap_or_default(),
    )
}

/// Bumped on every observed transition that can reshape Pending — an agent's
/// Working/Waiting/Done edge or a program record's blocked report arriving or
/// resolving — so open inbox views re-project. The projection itself is a
/// live read; the epoch only schedules the re-render, the way Caffeine's
/// signal does.
#[derive(Default)]
pub(crate) struct PendingEpoch(pub(crate) u64);
impl gpui_kit::Global for PendingEpoch {}

pub(crate) fn note_pending_changed(cx: &mut App) {
    cx.default_global::<PendingEpoch>().0 += 1;
}

/// Install the shell's source resolver: the deferred source query that used to
/// live inside `notify_app`, kept here because it reads the window registry,
/// the machine mirrors and the app entity.
pub(crate) fn install_source_resolver(cx: &mut App) {
    install_notification_source_resolver(
        cx,
        NotificationSourceResolver::new(|origin_pane, window, cx| {
            let app = source_app(window, cx);
            let app = match origin_pane {
                Some((workspace, _))
                    if app
                        .as_ref()
                        .is_none_or(|app| app.read(cx).workspace != workspace) =>
                {
                    if cx.has_global::<crate::ui::windows::WindowRegistry>() {
                        crate::ui::windows::WindowRegistry::app_for(cx, workspace)
                            .and_then(|app| app.upgrade())
                    } else {
                        None
                    }
                }
                _ => app,
            };
            let app = app?;
            if origin_pane.is_none() && root_app(window, cx).is_none() {
                Some(app.read(cx).notification_workspace_source(cx))
            } else {
                Some(app.read(cx).notification_source_for_pane(
                    origin_pane.map(|(_, pane)| pane),
                    window,
                    cx,
                ))
            }
        }),
    );
}

fn root_app(window: &Window, cx: &App) -> Option<Entity<LyrelApp>> {
    window
        .root::<gpui_kit::component::Root>()??
        .read(cx)
        .view()
        .clone()
        .downcast::<LyrelApp>()
        .ok()
}

fn source_app(window: &Window, cx: &App) -> Option<Entity<LyrelApp>> {
    let view = window
        .root::<gpui_kit::component::Root>()??
        .read(cx)
        .view()
        .clone();
    if let Ok(app) = view.clone().downcast::<LyrelApp>() {
        return Some(app);
    }
    let owner = view
        .downcast::<crate::ui::settings_window::SettingsWindow>()
        .ok()?
        .read(cx)
        .owner();
    owner
        .read(cx, |root: Entity<gpui_kit::component::Root>, cx| {
            root.read(cx).view().clone().downcast::<LyrelApp>().ok()
        })
        .ok()
        .flatten()
}

impl LyrelApp {
    #[cfg(test)]
    pub(crate) fn notification_source(&self, window: &Window, cx: &App) -> Source {
        self.notification_source_for_pane(None, window, cx)
    }

    pub(crate) fn notification_source_for_pane(
        &self,
        origin: Option<lyrel_core::core::pane_sidecar::ScopedPane>,
        window: &Window,
        cx: &App,
    ) -> Source {
        let tab = match origin {
            Some(pane) => self.tabs.iter().find(|tab| {
                tab.pane
                    .leaves()
                    .iter()
                    .any(|slot| crate::ui::pane_sidecar::scoped_pane(slot, cx) == Some(pane))
            }),
            None => self.tabs.get(self.tabs.active_index()),
        };
        let pane = origin.or_else(|| {
            tab.and_then(|tab| tab.pane.focused_or_first(window, cx))
                .and_then(|pane| crate::ui::pane_sidecar::scoped_pane(&PaneSlot::Ready(pane), cx))
        });
        let mut source = self.notification_workspace_source(cx);
        if let Some(tab) = tab {
            let title = tab.leaf_title(Some(window), cx);
            if !title.is_empty() {
                source.label.push_str(" · ");
                source.label.push_str(&title);
            }
        }
        if let Some(pane) = pane {
            source.label.push_str(&format!(" · #{}", pane.pane()));
        }
        source.tab = tab.map(|tab| tab.id());
        source.pane = pane.map(|pane| pane.pane());
        source.pane_owner = pane.map(|pane| pane.owner());
        source
    }

    fn notification_workspace_source(&self, cx: &App) -> Source {
        let workspace = crate::ui::machine_mirror::display_name_for(cx, self.workspace)
            .unwrap_or_else(|| self.workspace.to_string());
        Source {
            workspace: self.workspace,
            machine_workspace: WorkspaceStore::remote_ref(cx, self.workspace)
                .map(|remote| remote.workspace)
                .unwrap_or(self.workspace),
            host: WorkspaceStore::host_of(cx, self.workspace).0,
            tab: None,
            pane: None,
            pane_owner: None,
            label: format!("{} · {workspace}", self.remote_machine_label(cx)),
        }
    }

    /// Every pane currently waiting on the user in this window — the live
    /// Pending half of the inbox. An agent's `Waiting` and a program record's
    /// OSC 7501 `blocked` both qualify; a pane reporting both projects once,
    /// as the agent's wait. Exited terminals and settled states drop out at
    /// read time, so a row can only exist while its blocker is real.
    pub(crate) fn collect_pending(&self, items: &mut Vec<PendingItem>, cx: &App) {
        use crate::core::cli_agent::AgentStatus;
        use lyrel_core::core::program_status::ProgramState;
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                let view = leaf.read(cx);
                if view.terminal.exited {
                    continue;
                }
                let waiting = view
                    .agent_session()
                    .filter(|session| session.status == AgentStatus::Waiting);
                // The same live records the status bar reads: a blocked
                // program is a wait even when no agent is attached. The most
                // recently updated blocked record names the reason and age.
                let blocked = view.with_program_status(|status| {
                    status
                        .records()
                        .filter(|record| record.state() == ProgramState::Blocked)
                        .max_by_key(|record| record.updated())
                        .map(|record| (record.kind(), record.updated()))
                });
                let (state, since_ms) = match (waiting, blocked) {
                    (Some(session), _) => (PendingState::Waiting, session.since_ms),
                    // `updated` is authoritative Unix nanoseconds; Pending
                    // ages in milliseconds like the agent's `since_ms`.
                    (None, Some((kind, updated))) => {
                        (PendingState::Blocked(kind), Some(updated / 1_000_000))
                    }
                    (None, None) => continue,
                };
                let Some(pane) =
                    crate::ui::pane_sidecar::scoped_pane(&PaneSlot::Ready(leaf.clone()), cx)
                else {
                    continue;
                };
                items.push(PendingItem {
                    source: self.notification_source_for_leaf(tab, pane, cx),
                    state,
                    since_ms,
                });
            }
        }
    }

    /// The source a Pending row navigates to: same label ladder as
    /// `notification_source_for_pane`, but names the observed leaf without
    /// needing that leaf's window.
    pub(crate) fn notification_source_for_leaf(
        &self,
        tab: &crate::ui::tab::Tab,
        pane: lyrel_core::core::pane_sidecar::ScopedPane,
        cx: &App,
    ) -> Source {
        let mut source = self.notification_workspace_source(cx);
        let title = tab.leaf_title(None, cx);
        if !title.is_empty() {
            source.label.push_str(" · ");
            source.label.push_str(&title);
        }
        source.label.push_str(&format!(" · #{}", pane.pane()));
        source.tab = Some(tab.id());
        source.pane = Some(pane.pane());
        source.pane_owner = Some(pane.owner());
        source
    }

    pub(crate) fn notification_target(
        &self,
        source: &Source,
        cx: &App,
    ) -> Option<(usize, Option<PaneSlot>)> {
        if self.workspace != source.workspace
            || WorkspaceStore::host_of(cx, self.workspace).0 != source.host
        {
            return None;
        }
        if let Some(id) = source.pane {
            let mut matches = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| source.tab.is_none_or(|want| tab.id() == want))
                .flat_map(|(index, tab)| {
                    tab.pane.leaves().into_iter().map(move |pane| (index, pane))
                })
                .filter(|(_, pane)| {
                    crate::ui::pane_sidecar::scoped_pane(pane, cx).is_some_and(|pane| {
                        pane.pane() == id
                            && source.pane_owner.is_none_or(|owner| pane.owner() == owner)
                    })
                });
            let (index, pane) = matches.next()?;
            // Records without owner metadata must resolve uniquely; never focus
            // another daemon's session merely because it has the same number.
            matches.next().is_none().then_some((index, Some(pane)))
        } else if let Some(want) = source.tab {
            self.tabs
                .iter()
                .position(|tab| tab.id() == want)
                .map(|index| (index, None))
        } else {
            Some((self.tabs.active_index(), None))
        }
    }

    pub(crate) fn claim_notification_source(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let Some(source) = self.pending_notification_source.take() else {
            return;
        };
        if let Some((index, pane)) = self.notification_target(&source, cx) {
            self.tabs.select(index);
            if let Some(pane) = pane {
                if self.tabs[index].is_parked_slot(&pane, cx) {
                    self.preview_parked_target(pane.entity_id(), window, cx);
                } else {
                    self.focus_pane_command_target(pane.entity_id(), window, cx);
                }
            }
            cx.notify();
        } else {
            self.pending_notification_source = Some(source);
        }
    }
}

fn open_app(source: &Source, window: &Window, cx: &App) -> Option<Entity<LyrelApp>> {
    if let Some(app) = root_app(window, cx).filter(|app| app.read(cx).workspace == source.workspace)
    {
        return Some(app);
    }
    if !cx.has_global::<crate::ui::windows::WindowRegistry>() {
        return None;
    }
    crate::ui::windows::WindowRegistry::app_for(cx, source.workspace)?.upgrade()
}

fn stored_source_exists(source: &Source, cx: &App) -> bool {
    let remote = WorkspaceStore::remote_ref(cx, source.workspace);
    let (host, workspace) = remote
        .as_ref()
        .map(|remote| (remote.host_id(), remote.workspace))
        .unwrap_or((HostId::LOCAL, source.workspace));
    // A cached machine tree does not authorize reopening a forgotten/rebound
    // client bookmark: open_at_tab would otherwise fall back to a local shell.
    if host.0 != source.host || workspace != source.machine_workspace {
        return false;
    }
    if let Some(remote) = remote {
        let sidecar = crate::ui::machine_mirror::MachineMirrors::machine(cx, HostId::LOCAL)
            .and_then(|machine| {
                machine
                    .workspaces
                    .iter()
                    .filter_map(|workspace| workspace.foreign_view.as_ref())
                    .find(|foreign| foreign.remote() == &remote)
            });
        if let Some(sidecar) = sidecar {
            let mut panes = sidecar
                .tabs()
                .iter()
                .filter(|tab| source.tab.is_none_or(|want| tab.id == want))
                .flat_map(|tab| tab.root.pane_ids())
                .filter(|pane| {
                    source.pane.is_none_or(|id| pane.pane() == id)
                        && source.pane_owner.is_none_or(|owner| pane.owner() == owner)
                });
            if panes.next().is_some() && (source.pane.is_none() || panes.next().is_none()) {
                return true;
            }
        }
        if source.pane_owner == Some(lyrel_core::core::pane_sidecar::PaneOwner::ClientLocal) {
            return false;
        }
    }
    let Some(machine) = crate::ui::machine_mirror::MachineMirrors::machine(cx, HostId(source.host))
    else {
        return false;
    };
    let Some(workspace) = machine
        .workspaces
        .iter()
        .find(|workspace| workspace.id == source.machine_workspace)
    else {
        return false;
    };
    if source.tab.is_none() && source.pane.is_none() {
        return true;
    }
    workspace.tabs.iter().any(|tab| {
        source.tab.is_none_or(|want| tab.id == want)
            && source
                .pane
                .is_none_or(|pane| tab.root.pane_ids().contains(&pane))
    })
}

pub(crate) fn source_exists(source: &Source, window: &Window, cx: &App) -> bool {
    if let Some(app) = open_app(source, window, cx) {
        return app.read(cx).notification_target(source, cx).is_some();
    }
    stored_source_exists(source, cx)
}

pub(crate) fn navigate(source: &Source, window: &mut Window, cx: &mut App) -> bool {
    if !source_exists(source, window, cx) {
        return false;
    }
    if open_app(source, window, cx).is_none() {
        if let Some(tab) = source.tab {
            crate::ui::windows::open_at_tab(cx, source.workspace, tab);
        } else {
            crate::ui::windows::open(cx, Some(source.workspace));
        }
    }
    let Some(app) = open_app(source, window, cx) else {
        return false;
    };
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
    if root_app(window, cx).is_some_and(|app| app.read(cx).workspace == workspace) {
        focus(window, cx);
    } else if let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, workspace) {
        let _ = handle.update(cx, |_, window, cx| focus(window, cx));
    }
    true
}
