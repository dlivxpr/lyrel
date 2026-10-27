use std::collections::HashMap;

use gpui_kit::{App, Axis, Context, Window};

use crate::core::session::{
    Session, SessionAxis, SessionPane, SessionTab, WorkspaceId, WorkspaceStore,
};
use crate::core::window_state::WindowState;
use crate::ui::app::{LyrelApp, new_terminal, new_terminal_native, restore_agent_in_pane};
use crate::ui::host_registry::HostId;
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::tab::{Tab, TabSet};

/// Client-local recovery records. Live shells and durable layouts remain daemon-owned.
#[derive(Default)]
pub(crate) struct SessionLifecycle {
    closed: Vec<SessionTab>,
}

impl SessionLifecycle {
    pub(crate) fn remember_close(&mut self, snapshot: SessionTab) {
        const MAX_CLOSED_TABS: usize = 20;
        self.closed.push(snapshot);
        if self.closed.len() > MAX_CLOSED_TABS {
            self.closed.remove(0);
        }
    }

    pub(crate) fn take_closed(&mut self) -> Option<SessionTab> {
        self.closed.pop()
    }

    pub(crate) fn newest_closed(&self) -> Option<&SessionTab> {
        self.closed.last()
    }

    pub(crate) fn host_changed(&mut self, previous: HostId, next: HostId) {
        if crate::core::session::crosses_machines(previous, next) {
            self.closed.clear();
        }
    }

    /// The successor must be settled before releasing any live attachment.
    pub(crate) fn hibernate_successor(
        tabs: &TabSet,
        index: usize,
        owner: WorkspaceId,
        cx: &App,
    ) -> Option<usize> {
        let tab = tabs.get(index)?;
        if !crate::ui::tree_sync::can_hibernate_on(cx, owner) {
            return None;
        }
        let remote = WorkspaceStore::all(cx)
            .get(owner)
            .is_some_and(|w| w.is_remote());
        tab.is_hibernatable(remote, cx)
            .then(|| tabs.awake_besides(index))
            .flatten()
    }

    pub(crate) fn sleep(tabs: &mut TabSet, index: usize, window: &Window, cx: &App) -> bool {
        let Some(tab) = tabs.get_mut(index).filter(|tab| !tab.is_asleep()) else {
            return false;
        };
        let (view, home) = tab.label_view(Some(window), cx);
        let layout = pane_to_session(&tab.pane, cx);
        tab.sleep(layout, view, home);
        true
    }

    pub(crate) fn wake(
        tabs: &mut TabSet,
        index: usize,
        workspace: Option<crate::terminal::PaneWorkspace>,
        owner: WorkspaceId,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> bool {
        let Some(tab) = tabs.get_mut(index) else {
            return true;
        };
        let Some(layout) = tab.asleep_layout() else {
            return true;
        };
        let layout = redial_native_ssh(layout);
        let mut presentation = tab.presentation().clone();
        // Deliberately no listing: stopped panes try attach then fresh spawn.
        let restore = SessionRestore {
            workspace,
            owner,
            font_size,
            alive: None,
        };
        let Some(pane) = restore.pane(&layout, window, cx, &mut presentation) else {
            return false;
        };
        tab.finish_wake(pane, presentation);
        true
    }
}

/// One restore attempt owns its route and liveness evidence. A failed listing
/// is not an empty listing, and client-local SSH never borrows remote IDs.
pub(crate) struct SessionRestore {
    workspace: Option<crate::terminal::PaneWorkspace>,
    owner: WorkspaceId,
    font_size: f32,
    alive: Option<HashMap<u64, Option<String>>>,
}

impl SessionRestore {
    fn new(
        workspace: Option<crate::terminal::PaneWorkspace>,
        owner: WorkspaceId,
        font_size: f32,
    ) -> Self {
        let alive = alive_panes_on(&crate::terminal::PaneRoute::for_workspace(
            workspace.as_ref(),
        ));
        Self {
            workspace,
            owner,
            font_size,
            alive,
        }
    }

    pub(crate) fn reopen(
        workspace: Option<crate::terminal::PaneWorkspace>,
        owner: WorkspaceId,
        font_size: f32,
        mut snapshot: SessionTab,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> Result<Tab, SessionTab> {
        let restore = Self::new(workspace, owner, font_size);
        match restore.pane(&snapshot.pane, window, cx, &mut snapshot.presentation) {
            Some(pane) => Ok(Tab::reopened(snapshot, pane)),
            None => Err(snapshot),
        }
    }
}

pub(crate) fn record_window(owner: WorkspaceId, tabs: &TabSet, window: WindowState, cx: &mut App) {
    for terminal in tabs.iter().flat_map(|tab| tab.pane.terminals()) {
        let view = terminal.read(cx);
        if let Some(recorded) = view.owner_workspace().filter(|recorded| *recorded != owner) {
            log::error!(
                "save_session: window of workspace {owner} is recording pane {} \
                 that was created for workspace {recorded} — cross-workspace \
                 write detected, please report this",
                view.pane_id,
            );
        }
    }
    WorkspaceStore::record_geometry(cx, owner, window);
}

pub(crate) enum WindowDeparture {
    Switch,
    Close,
}

/// Leaving a window is not a stop request. Only an informed empty workspace
/// may be removed; failed or partial hydration never grants that authority.
pub(crate) fn depart_window(
    owner: WorkspaceId,
    tabs: &TabSet,
    departure: WindowDeparture,
    cx: &mut App,
) {
    let closing = matches!(departure, WindowDeparture::Close);
    let answered = !closing || WorkspaceStore::machine_is_connected(cx, owner);
    if tabs.is_empty() && answered && crate::ui::tree_sync::workspace_is_disposable(cx, owner) {
        crate::ui::tree_sync::fire_workspace_op(cx, owner, |workspace| {
            lyrel_core::daemon::control::ControlRequest::WorkspaceRemove { workspace }
        });
        WorkspaceStore::remove(cx, owner);
    } else {
        WorkspaceStore::close_window(cx, owner);
    }
    if closing {
        crate::ui::windows::WindowRegistry::unregister(cx, owner);
    }
    crate::ui::tree_sync::forget(cx, owner);
    if closing {
        crate::ui::windows::refresh_menu(cx);
    }
}

pub(crate) fn retire_last_window(cx: &mut App) {
    if !cx.global::<crate::core::config::Config>().show_tray_icon || !crate::ui::tray::icon_is_up()
    {
        cx.spawn(async move |cx| {
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    }
}
pub(crate) fn tab_to_session(tab: &Tab, cx: &App) -> SessionTab {
    SessionTab {
        name: tab.name().clone(),
        pane: match tab.asleep_layout() {
            Some(layout) => layout.clone(),
            None => pane_to_session(&tab.pane, cx),
        },
        presentation: tab.presentation().clone(),
        group: tab.group.get(),
        last_auto: tab.auto_group.borrow().clone(),
        tree_id: None,
        hibernated: false,
        asleep_view: None,
    }
}
fn pane_to_session(pane: &Pane, cx: &App) -> SessionPane {
    match pane {
        Pane::Leaf(PaneSlot::Connecting(pending)) => pending.read(cx).session(),
        Pane::Leaf(PaneSlot::Ready(view)) => {
            let view = view.read(cx);
            SessionPane::Leaf {
                // A native SSH leaf keeps the far shell's directory, so a
                // sleeping SSH tab wakes where it was: the redial that wakes it
                // takes this as its start directory on the same host, and the
                // tree record it seeds holds the far directory for such a pane
                // anyway. Any other remote pane's cwd is on a host no restore
                // dials, so it is still left out.
                cwd: match view.ssh_spec() {
                    Some(_) => view.cwd(),
                    None => view.spawnable_cwd(),
                },
                pane_id: Some(view.pane_id),
                // `None` for a pane this window attached to rather than
                // spawned: it never knew what was on the other end. The tree
                // does — the daemon records it — and that is what a restore
                // reads, so the gap here costs nothing it can see.
                shell: view.shell_spec(),
                ssh_spec: view.ssh_spec(),
                agent: view.agent(),
                agent_session_id: view.agent_session().and_then(|s| s.session_id),
                agent_launch_argv: view.agent().and_then(|agent| {
                    view.agent_session().and_then(|state| {
                        lyrel_core::host::session_operations::launch_context(agent, &state)
                    })
                }),
                agent_session_scope: view.agent_session().and_then(|s| {
                    s.readout
                        .session_scope
                        .as_ref()
                        .map(lyrel_core::host::session_operations::durable_scope)
                }),
            }
        }
        Pane::Split {
            axis, a, b, ratio, ..
        } => SessionPane::Split {
            axis: match axis {
                Axis::Horizontal => SessionAxis::Horizontal,
                Axis::Vertical => SessionAxis::Vertical,
            },
            ratio: ratio.get(),
            a: Box::new(pane_to_session(a, cx)),
            b: Box::new(pane_to_session(b, cx)),
        },
        Pane::Empty => SessionPane::Leaf {
            cwd: None,
            pane_id: None,
            shell: None,
            ssh_spec: None,
            agent: None,
            agent_session_id: None,
            agent_launch_argv: None,
            agent_session_scope: None,
        },
    }
}

/// The daemon's account of which panes are alive, or `None` when it could not
/// be asked at all.
///
/// The distinction is the point: a pane absent from a *successful* listing is
/// genuinely gone and may be respawned, while a failed `List` says nothing
/// about any pane. Flattening the failure into an empty map made one transient
/// RPC error read as "every pane is dead", and the restore then spawned fresh
/// shells over all of them — the same destruction-by-inference this file's
/// restore path is built to avoid.
fn alive_panes_on(
    route: &crate::terminal::PaneRoute,
) -> Option<std::collections::HashMap<u64, Option<String>>> {
    if !matches!(route, crate::terminal::PaneRoute::Local) {
        return Some(std::collections::HashMap::new());
    }
    match crate::terminal::RemoteTerminal::try_list_panes_on(route) {
        Ok(list) => Some(
            list.into_iter()
                .filter(|p| p.alive)
                .map(|p| (p.pane_id, p.owner))
                .collect(),
        ),
        Err(e) => {
            log::warn!("could not list panes ({e}); leaving each attach to decide");
            None
        }
    }
}

/// Whether this window may stand on `id` — as the pane it attaches to, or as
/// the dead predecessor whose screen a fresh pane opens showing.
///
/// Liveness deliberately does not come into it, and that is the whole point.
/// A pane missing from the listing is usually one whose daemon has just
/// restarted, which is exactly when its stored screen is worth asking for.
/// Ruling the id out there threw away the only thing that could ask: the
/// window spawned a pane that had never heard of a predecessor, so no attach
/// was tried, no restore was requested, and the screen the daemon still had on
/// disk was swept a tick later, unread.
///
/// There used to be a second predicate here that also required the id to be
/// listed, and the attach site consulted it. Nothing does now: the attach is
/// simply tried, and a pane that really is gone fails it and falls through to
/// the fresh spawn — the same outcome the listing was consulted to predict,
/// reached by asking the daemon instead of guessing ahead of it.
///
/// Ownership does come into it. Another workspace's pane is not this window's
/// to attach to, and its screen is not this window's to show.
fn pane_free_for(
    alive: Option<&std::collections::HashMap<u64, Option<String>>>,
    id: u64,
    owner: crate::core::session::WorkspaceId,
) -> bool {
    let Some(alive) = alive else {
        // No listing to consult. Attaching is the safe guess in both
        // directions: if the daemon is really unreachable the attach fails and
        // the pane falls to the fresh-spawn path anyway, while spawning fresh
        // on a hunch destroys a session that was merely hard to reach.
        return true;
    };
    match alive.get(&id) {
        None => true,
        Some(None) => true,
        Some(Some(recorded)) => {
            // Only a workspace id is a claim. Anything else is a client
            // stamping its own name rather than claiming workspace ownership.
            // Refusing on it strands CLI-created panes, respawning over a live
            // shell the tree just told us belongs here.
            let Ok(recorded) = recorded.parse::<crate::core::session::WorkspaceId>() else {
                return true;
            };
            let ours = recorded == owner;
            if !ours {
                log::warn!(
                    "restore: pane {id} is owned by workspace {recorded}, not {owner}; \
                     spawning fresh instead of attaching to it"
                );
            }
            ours
        }
    }
}

impl SessionRestore {
    pub(crate) fn tabs(
        workspace: Option<crate::terminal::PaneWorkspace>,
        owner: WorkspaceId,
        session: Option<Session>,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> (Vec<Tab>, usize, usize) {
        let Some(session) = session.filter(|s| !s.tabs.is_empty()) else {
            return (Vec::new(), 0, 0);
        };
        let restore = Self::new(workspace, owner, font_size);
        let workspace = restore.workspace.as_ref();
        let mut tabs: Vec<Tab> = Vec::with_capacity(session.tabs.len());
        let mut dropped = 0usize;
        let home = crate::ui::path_display::home_for_host(
            cx,
            workspace.map_or(HostId::LOCAL, |w| w.target.host_id()),
        );
        for (index, st) in session.tabs.iter().enumerate() {
            // Asleep stays asleep: nothing is spawned or attached for it, which is
            // the point. The one exception is the tab the window opens onto — a
            // tab on screen is awake, so that one is woken here, by the same
            // restore every other tab is getting.
            if st.hibernated && index != session.active {
                tabs.push(Tab::from_asleep(st, home.clone()));
                continue;
            }
            let mut presentation = st.presentation.clone();
            let Some(pane) = restore.pane(&st.pane, window, cx, &mut presentation) else {
                // The layout coming back is the product's headline claim, so a tab
                // that quietly does not is worth a sentence rather than a log line.
                log::error!("dropping a restored tab: no pane in it could be started");
                dropped += 1;
                continue;
            };
            tabs.push(Tab::from_session(st, pane, presentation));
        }
        let active = session.active.min(tabs.len().saturating_sub(1));
        (tabs, active, dropped)
    }
}

/// A sleeping layout as a wake should read it: a native SSH pane dials its
/// host again rather than standing on its old id. That id belongs to a pane
/// whose connection is gone, and the restore reading it would put a local
/// shell in its place.
fn redial_native_ssh(layout: &SessionPane) -> SessionPane {
    match layout {
        SessionPane::Leaf {
            ssh_spec: Some(_), ..
        } => {
            let mut leaf = layout.clone();
            if let SessionPane::Leaf { pane_id, .. } = &mut leaf {
                *pane_id = None;
            }
            leaf
        }
        SessionPane::Leaf { .. } => layout.clone(),
        SessionPane::Split { axis, ratio, a, b } => SessionPane::Split {
            axis: *axis,
            ratio: *ratio,
            a: Box::new(redial_native_ssh(a)),
            b: Box::new(redial_native_ssh(b)),
        },
    }
}

/// Whether a native SSH pane can be attached to by its old id: only when the
/// server lists it. A failed attach falls through to a fresh *local* shell,
/// and after a server restart that put the user's own machine behind a tab
/// that had been — and still looked like — a session on another one. Without
/// a listing the host is dialled again: a second connection is recoverable,
/// typing into the wrong machine is not.
fn native_ssh_pane_alive(
    alive: Option<&std::collections::HashMap<u64, Option<String>>>,
    id: u64,
) -> bool {
    alive.is_some_and(|alive| alive.contains_key(&id))
}

fn leaf_shares_the_window_daemon(window_is_remote: bool, leaf_is_native_ssh: bool) -> bool {
    !(window_is_remote && leaf_is_native_ssh)
}

impl SessionRestore {
    fn pane(
        &self,
        sp: &SessionPane,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
        presentation: &mut lyrel_core::core::machine::TabPresentation,
    ) -> Option<Pane> {
        let workspace = self.workspace.as_ref();
        let owner = self.owner;
        let font_size = self.font_size;
        let alive = self.alive.as_ref();
        match sp {
            SessionPane::Leaf {
                cwd,
                pane_id,
                shell,
                ssh_spec,
                agent,
                agent_session_id,
                agent_launch_argv,
                agent_session_scope,
            } => {
                let same_daemon =
                    leaf_shares_the_window_daemon(workspace.is_some(), ssh_spec.is_some());
                let restore = match workspace.is_some() {
                    true => (*pane_id).filter(|_| same_daemon),
                    // Not `pane_attachable`: a dead pane's id is what the restore
                    // is keyed on, so it has to survive being dead. The attach is
                    // still attempted first and still gives way to a fresh spawn.
                    false => (*pane_id).filter(|id| {
                        same_daemon
                            && pane_free_for(alive, *id, owner)
                            && (ssh_spec.is_none() || native_ssh_pane_alive(alive, *id))
                    }),
                };
                if restore.is_none() {
                    if let Some(spec) = ssh_spec.clone() {
                        let resolved = crate::ui::ssh_connect::resolve_persisted_ssh_spec(spec, cx);
                        // A native SSH leaf's recorded cwd is the far shell's own
                        // report, so it is where the redialled shell starts.
                        let view = new_terminal_native(
                            font_size,
                            owner,
                            cwd.clone(),
                            resolved,
                            window,
                            cx,
                        );
                        if same_daemon {
                            remap_presentation_slot(presentation, *pane_id, &view, cx);
                        }
                        return Some(Pane::leaf(view));
                    }
                }
                // Past here the leaf comes back as a shell on the window's own
                // machine: reattached, or spawned fresh in place of one that is
                // gone. A native SSH leaf's cwd is a path on the far host, so a
                // fresh shell standing in for one must not start in it here.
                let local_cwd = match ssh_spec {
                    Some(_) => None,
                    None => cwd.clone(),
                };
                let view = match new_terminal(
                    workspace.cloned(),
                    Some(owner),
                    font_size,
                    local_cwd,
                    restore,
                    shell.clone(),
                    window,
                    cx,
                ) {
                    Ok(view) => view,
                    Err(e) => {
                        log::error!("restoring pane failed: {e}");
                        return None;
                    }
                };
                match &view {
                    PaneSlot::Ready(terminal) if !terminal.read(cx).restored() => {
                        restore_agent_in_pane(
                            *agent,
                            agent_session_id.clone(),
                            agent_launch_argv.clone(),
                            agent_session_scope.clone(),
                            terminal.clone(),
                            window,
                            cx,
                        );
                    }
                    PaneSlot::Ready(_) => {}
                    PaneSlot::Connecting(pending) => {
                        pending.update(cx, |pending, _| {
                            pending.resume_agent(
                                *agent,
                                agent_session_id.clone(),
                                agent_launch_argv.clone(),
                                agent_session_scope.clone(),
                                false,
                            );
                        });
                    }
                }
                if same_daemon {
                    remap_presentation_slot(presentation, *pane_id, &view, cx);
                }
                Some(Pane::leaf(view))
            }
            SessionPane::Split { axis, ratio, a, b } => {
                let axis = match axis {
                    SessionAxis::Horizontal => Axis::Horizontal,
                    SessionAxis::Vertical => Axis::Vertical,
                };
                match (
                    self.pane(a, window, cx, presentation),
                    self.pane(b, window, cx, presentation),
                ) {
                    (Some(a), Some(b)) => Some(Pane::split_node(axis, *ratio, a, b)),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }
}

fn remap_presentation_slot(
    presentation: &mut lyrel_core::core::machine::TabPresentation,
    old: Option<u64>,
    slot: &PaneSlot,
    cx: &App,
) {
    let Some(old) = old else {
        return;
    };
    let new = match slot {
        PaneSlot::Ready(view) => Some(view.read(cx).pane_id),
        PaneSlot::Connecting(view) => view.read(cx).restore_pane(),
    };
    presentation.remap_panes(|pane| if pane == old { new } else { Some(pane) });
}
/// Split a closing tab's panes into the ones its machine stops for it and the
/// ones this window ends itself: `(pane id, route, is native SSH)` in, and the
/// same pairs out, machine-held first.
///
/// A native SSH pane in a remote window lives in this computer's daemon, not
/// on the window's machine — the tree never holds it (see
/// `leaf_shares_the_window_daemon`) — so the machine cannot stop it and it is
/// ended from here. Without `remote_window` every pane is the machine's.
pub(crate) fn split_machine_held(
    remote_window: bool,
    leaves: &[(u64, crate::terminal::PaneRoute, bool)],
) -> (
    Vec<(u64, crate::terminal::PaneRoute)>,
    Vec<(u64, crate::terminal::PaneRoute)>,
) {
    let (held, here): (Vec<_>, Vec<_>) = leaves
        .iter()
        .cloned()
        .partition(|(_, _, native_ssh)| leaf_shares_the_window_daemon(remote_window, *native_ssh));
    let strip = |v: Vec<(u64, crate::terminal::PaneRoute, bool)>| {
        v.into_iter().map(|(id, route, _)| (id, route)).collect()
    };
    (strip(held), strip(here))
}

#[cfg(test)]
#[path = "session_lifecycle/tests_restore.rs"]
mod tests_restore;
