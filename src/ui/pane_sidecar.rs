//! The local daemon's durable composition of a remote workspace and local SSH sessions.

use std::collections::{HashMap, HashSet};

use gpui_kit::{App, Context, Global, Window};
use lyrel_core::core::machine::{PaneNode, Tab as TreeTab, TabId, Workspace};
use lyrel_core::core::pane_sidecar::{ForeignWorkspace, ScopedPane, SidecarUpdate};
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};

use crate::core::session::{RemoteRef, WorkspaceId, WorkspaceStore};
use crate::ui::app::LyrelApp;
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::tab::Tab;

#[derive(Default)]
struct SidecarState {
    known: Option<Workspace>,
    baseline: HashMap<TabId, TreeTab<ScopedPane>>,
    pending: Vec<SidecarUpdate>,
    sent: Vec<SidecarUpdate>,
    seeds: HashMap<u64, lyrel_core::core::machine::PaneSeed>,
    records: std::sync::Arc<HashMap<u64, lyrel_core::core::machine::PaneRecord>>,
    loaded: bool,
    pulling: bool,
    inflight: bool,
}

#[derive(Default)]
struct PaneSidecars(HashMap<RemoteRef, SidecarState>);
impl Global for PaneSidecars {}

pub(crate) fn scoped_pane(slot: &PaneSlot, cx: &App) -> Option<ScopedPane> {
    match slot {
        PaneSlot::Ready(view) => {
            let view = view.read(cx);
            Some(if view.pane_route().is_local() {
                ScopedPane::client_local(view.pane_id)
            } else {
                ScopedPane::workspace(view.pane_id)
            })
        }
        PaneSlot::Connecting(pending) => {
            let pending = pending.read(cx);
            let pane = pending.restore_pane()?;
            Some(if pending.workspace().is_some() {
                ScopedPane::workspace(pane)
            } else {
                ScopedPane::client_local(pane)
            })
        }
    }
}

fn logical_node(pane: &Pane, cx: &App) -> Option<PaneNode<ScopedPane>> {
    match pane {
        Pane::Empty => None,
        Pane::Leaf(slot) => scoped_pane(slot, cx).map(|pane| PaneNode::Leaf { pane }),
        Pane::Split {
            axis, ratio, a, b, ..
        } => match (logical_node(a, cx), logical_node(b, cx)) {
            (Some(a), Some(b)) => Some(PaneNode::Split {
                axis: if *axis == gpui_kit::Axis::Horizontal {
                    lyrel_core::core::machine::Axis::Horizontal
                } else {
                    lyrel_core::core::machine::Axis::Vertical
                },
                ratio: ratio.get(),
                a: Box::new(a),
                b: Box::new(b),
            }),
            (a, b) => a.or(b),
        },
    }
}

pub(crate) fn snapshot(tab: &Tab, cx: &App) -> Option<TreeTab<ScopedPane>> {
    let root = logical_node(&tab.pane, cx)?;
    let mut presentation = tab.scoped_presentation().cloned().unwrap_or_else(|| {
        tab.presentation()
            .map_panes(|pane| Some(ScopedPane::workspace(pane)))
    });
    presentation.remap_panes(|pane| root.contains(pane).then_some(pane));
    Some(TreeTab {
        id: tab.id(),
        name: tab.name().clone(),
        group: tab.group.get(),
        last_auto: tab.auto_group.borrow().clone(),
        root,
        presentation,
        hibernated: false,
    })
}

pub(crate) fn mixed(tab: &Tab, cx: &App) -> bool {
    tab.scoped_presentation().is_some()
        || tab
            .pane
            .leaves()
            .iter()
            .any(|slot| scoped_pane(slot, cx).is_some_and(|pane| pane.owner().is_client_local()))
}

fn pane_from_existing(
    node: &PaneNode<ScopedPane>,
    slots: &mut HashMap<ScopedPane, PaneSlot>,
) -> Option<Pane> {
    match node {
        PaneNode::Leaf { pane } => slots.remove(pane).map(Pane::Leaf),
        PaneNode::Split { axis, ratio, a, b } => {
            match (pane_from_existing(a, slots), pane_from_existing(b, slots)) {
                (Some(a), Some(b)) => Some(Pane::split_node(
                    if *axis == lyrel_core::core::machine::Axis::Horizontal {
                        gpui_kit::Axis::Horizontal
                    } else {
                        gpui_kit::Axis::Vertical
                    },
                    *ratio,
                    a,
                    b,
                )),
                (a, b) => a.or(b),
            }
        }
    }
}


pub(crate) fn install_presentation(tab: &mut Tab, logical: TreeTab<ScopedPane>, cx: &App) -> bool {
    if logical_node(&tab.pane, cx).as_ref() != Some(&logical.root) {
        let mut slots = tab
            .pane
            .leaves()
            .into_iter()
            .filter_map(|slot| scoped_pane(&slot, cx).map(|key| (key, slot)))
            .collect();
        let Some(pane) = pane_from_existing(&logical.root, &mut slots) else {
            return false;
        };
        tab.pane = pane;
    }
    tab.adopt_scoped_presentation(logical.presentation);
    true
}


pub(crate) fn retain_client_local(tab: &mut Tab, cx: &App) -> bool {
    let Some(local) = snapshot(tab, cx)
        .and_then(|tab| tab.map_panes(|pane| pane.owner().is_client_local().then_some(pane)))
    else {
        return false;
    };
    install_presentation(tab, local, cx)
}

pub(crate) fn note_primary_tab(tab: &mut Tab, primary: &TreeTab, cx: &App) {
    if tab.scoped_presentation().is_none() {
        return;
    }
    let before = tab.presentation().pip().map(|pip| pip.pane());
    let after = primary.presentation.pip().map(|pip| pip.pane());
    let Some(mut logical) = snapshot(tab, cx) else {
        return;
    };
    let initial = logical.presentation.clone();
    let fallback = logical.root.pane_ids().first().copied();
    for entry in tab.presentation().parked() {
        if !primary.presentation.is_parked(entry.pane()) {
            logical.restore_parked(ScopedPane::workspace(entry.pane()), fallback);
        }
    }
    for entry in primary.presentation.parked() {
        if !tab.presentation().parked().contains(entry) {
            let restore = entry.map_panes(|pane| Some(ScopedPane::workspace(pane)));
            logical.park_with_restore(ScopedPane::workspace(entry.pane()), restore);
        }
    }
    let current = logical.presentation.pip().map(|pip| pip.pane());
    if before != after || primary.presentation.pip() != tab.presentation().pip() {
        if let Some(after) = after {
            let restore = primary
                .presentation
                .pip()
                .and_then(|entry| entry.map_panes(|pane| Some(ScopedPane::workspace(pane))));
            logical.set_pip_with_restore(Some(ScopedPane::workspace(after)), fallback, restore);
        } else if current
            .is_some_and(|pane| !pane.owner().is_client_local() && Some(pane.pane()) == before)
        {
            logical.set_pip(None, fallback);
        }
    }
    if logical
        .presentation
        .pip()
        .is_some_and(|pip| !pip.pane().owner().is_client_local())
    {
        logical
            .presentation
            .set_pip_geometry(primary.presentation.pip_geometry());
    }
    if logical.presentation != initial {
        install_presentation(tab, logical, cx);
    }
}

pub(crate) fn sync_window(app: &LyrelApp, cx: &mut App) {
    let Some(remote) = WorkspaceStore::remote_ref(cx, app.workspace) else {
        return;
    };
    let snapshots: Vec<_> = app
        .tabs
        .iter()
        .filter(|tab| mixed(tab, cx))
        .filter_map(|tab| snapshot(tab, cx))
        .collect();
    let all_snapshots: Vec<_> = app
        .tabs
        .iter()
        .filter_map(|tab| snapshot(tab, cx))
        .collect();
    let (loaded, changed) = {
        let state = cx
            .default_global::<PaneSidecars>()
            .0
            .entry(remote.clone())
            .or_default();
        let before = state.pending.len();
        if state.loaded {
            let known_keys: HashMap<_, _> = state
                .baseline
                .iter()
                .flat_map(|(tab, value)| {
                    value
                        .root
                        .pane_ids()
                        .into_iter()
                        .map(move |pane| (pane, *tab))
                })
                .collect();
            for desired in &all_snapshots {
                if state.baseline.contains_key(&desired.id) {
                    continue;
                }
                if desired
                    .root
                    .pane_ids()
                    .iter()
                    .any(|pane| known_keys.get(pane).is_some_and(|tab| *tab != desired.id))
                {
                    state
                        .pending
                        .push(SidecarUpdate::new(None, desired.clone()).with_transfer());
                    state.baseline.insert(desired.id, desired.clone());
                }
            }
        }
        for desired in snapshots {
            let transferred = desired.root.pane_ids().iter().any(|pane| {
                state
                    .baseline
                    .iter()
                    .any(|(tab, value)| *tab != desired.id && value.root.contains(*pane))
            });
            let base = state.baseline.get(&desired.id);
            if base == Some(&desired) {
                continue;
            }
            let update = SidecarUpdate::new(base.cloned(), desired.clone());
            state.pending.push(if transferred {
                update.with_transfer()
            } else {
                update
            });
            state.baseline.insert(desired.id, desired);
        }
        (state.loaded, state.pending.len() != before)
    };
    if changed {
        let seeds: Vec<_> = app
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.leaves())
            .filter(|slot| scoped_pane(slot, cx).is_some_and(|pane| pane.owner().is_client_local()))
            .filter_map(|slot| crate::ui::tree_sync::projection::seed_for_slot(&slot, cx))
            .collect();
        let state = cx
            .default_global::<PaneSidecars>()
            .0
            .entry(remote.clone())
            .or_default();
        for seed in seeds {
            state.seeds.insert(seed.pane, seed);
        }
    }
    if !loaded {
        refresh(cx, remote.clone());
    }
    pump(cx, remote);
}

pub(crate) fn intend_close(cx: &mut App, workspace: WorkspaceId, tab: TabId) {
    let Some(remote) = WorkspaceStore::remote_ref(cx, workspace) else {
        return;
    };
    let state = cx
        .default_global::<PaneSidecars>()
        .0
        .entry(remote.clone())
        .or_default();
    if state.baseline.remove(&tab).is_some() || !state.loaded {
        state.pending.push(SidecarUpdate::close_tab(tab));
    }
    pump(cx, remote);
}

pub(crate) fn on_sidecar_snapshot(
    cx: &mut App,
    snapshot: lyrel_core::core::pane_sidecar::SidecarSnapshot,
) {
    let (workspace, records) = snapshot.into_parts();
    let Some(foreign) = &workspace.foreign_view else {
        return;
    };
    let remote = foreign.remote().clone();
    accept(cx, remote, Some(workspace), Some(records));
}

pub(crate) fn on_sidecar_delta(cx: &mut App, workspace: &Workspace) {
    let Some(view) = &workspace.foreign_view else {
        return;
    };
    let remote = view.remote().clone();
    accept(cx, remote.clone(), Some(workspace.clone()), None);
    // Refresh the matching records before interpreting a missing local session.
    refresh(cx, remote);
}

pub(crate) fn on_local_link_up(cx: &mut App) {
    let remotes: Vec<_> = crate::ui::windows::WindowRegistry::open_windows(cx)
        .into_iter()
        .filter_map(|(workspace, _)| WorkspaceStore::remote_ref(cx, workspace))
        .collect();
    for remote in remotes {
        refresh(cx, remote.clone());
        pump(cx, remote);
    }
}

fn refresh(cx: &mut App, remote: RemoteRef) {
    let Some(client) = crate::ui::local_link::LocalLink::client(cx) else {
        return;
    };
    let state = cx
        .default_global::<PaneSidecars>()
        .0
        .entry(remote.clone())
        .or_default();
    if state.pulling {
        return;
    }
    state.pulling = true;
    cx.spawn(async move |cx| {
        let request = remote.clone();
        let result = cx
            .background_executor()
            .spawn(async move {
                let (workspace, records) =
                    match client.call(ControlRequest::WorkspaceSidecarGet { remote: request })? {
                        ReplyOk::WorkspaceSidecar(Some(snapshot)) => {
                            let (workspace, records) = snapshot.into_parts();
                            (Some(workspace), records)
                        }
                        ReplyOk::WorkspaceSidecar(None) => (None, Vec::new()),
                        other => {
                            return Err(std::io::Error::other(format!(
                                "sidecar get answered {other:?}"
                            )));
                        }
                    };
                Ok((workspace, records))
            })
            .await;
        cx.update(|cx| {
            cx.default_global::<PaneSidecars>()
                .0
                .entry(remote.clone())
                .or_default()
                .pulling = false;
            match result {
                Ok((Some(workspace), records)) => on_sidecar_snapshot(
                    cx,
                    lyrel_core::core::pane_sidecar::SidecarSnapshot::new(workspace, records),
                ),
                Ok((None, records)) => accept(cx, remote.clone(), None, Some(records)),
                Err(error) => log::warn!("could not load local-owner pane composition: {error}"),
            }
            pump(cx, remote);
        });
    })
    .detach();
}

fn pump(cx: &mut App, remote: RemoteRef) {
    if !cx
        .try_global::<PaneSidecars>()
        .and_then(|sidecars| sidecars.0.get(&remote))
        .is_some_and(|state| state.loaded && !state.inflight && !state.pending.is_empty())
    {
        return;
    }
    let Some(client) = crate::ui::local_link::LocalLink::client(cx) else {
        return;
    };
    let state = cx
        .default_global::<PaneSidecars>()
        .0
        .entry(remote.clone())
        .or_default();
    if !state.loaded || state.inflight || state.pending.is_empty() {
        return;
    }
    let seeds = state.seeds.values().cloned().collect();
    state.inflight = true;
    state.sent = std::mem::take(&mut state.pending);
    let updates = state.sent.clone();
    cx.spawn(async move |cx| {
        let request = remote.clone();
        let result = cx
            .background_executor()
            .spawn(async move {
                let (workspace, records) =
                    match client.call(ControlRequest::WorkspaceSidecarApply {
                        remote: request,
                        updates,
                        panes: seeds,
                    })? {
                        ReplyOk::WorkspaceSidecar(Some(snapshot)) => snapshot.into_parts(),
                        other => {
                            return Err(std::io::Error::other(format!(
                                "sidecar apply answered {other:?}"
                            )));
                        }
                    };
                Ok((workspace, records))
            })
            .await;
        cx.update(|cx| {
            let state = cx
                .default_global::<PaneSidecars>()
                .0
                .entry(remote.clone())
                .or_default();
            state.inflight = false;
            match result {
                Ok((workspace, records)) => {
                    state.sent.clear();
                    on_sidecar_snapshot(
                        cx,
                        lyrel_core::core::pane_sidecar::SidecarSnapshot::new(workspace, records),
                    );
                    pump(cx, remote);
                }
                Err(error) => {
                    let mut retry = std::mem::take(&mut state.sent);
                    retry.append(&mut state.pending);
                    state.pending = retry;
                    log::warn!("could not persist local-owner pane composition: {error}");
                }
            }
        });
    })
    .detach();
}

fn accept(
    cx: &mut App,
    remote: RemoteRef,
    workspace: Option<Workspace>,
    records: Option<Vec<lyrel_core::core::machine::PaneRecord>>,
) {
    let state = cx
        .default_global::<PaneSidecars>()
        .0
        .entry(remote.clone())
        .or_default();
    if let Some(workspace) = &workspace {
        let revision = workspace
            .foreign_view
            .as_ref()
            .map_or(0, ForeignWorkspace::revision);
        let previous = state
            .known
            .as_ref()
            .and_then(|workspace| workspace.foreign_view.as_ref())
            .map_or(0, ForeignWorkspace::revision);
        if state.known.is_some() && revision < previous {
            return;
        }
    }
    state.loaded = true;
    let authoritative_records = workspace.is_some() && records.is_some();
    if let Some(records) = records {
        state.records =
            std::sync::Arc::new(records.into_iter().map(|pane| (pane.id, pane)).collect());
    }
    if let Some(workspace) = workspace {
        state.known = Some(workspace);
    }
    let Some(mut effective) = state.known.clone() else {
        return;
    };
    let Some(foreign) = effective.foreign_view.as_mut() else {
        return;
    };
    // Only a matching daemon snapshot can prove a local session was closed.
    // Liveness, missing views and remote connection state cannot. Compute this
    // before replay so locally added, not-yet-acknowledged sessions survive.
    let closed_local: HashSet<_> = if authoritative_records {
        foreign
            .tabs()
            .iter()
            .flat_map(|tab| tab.root.pane_ids())
            .filter(|pane| {
                pane.owner().is_client_local() && !state.records.contains_key(&pane.pane())
            })
            .collect()
    } else {
        HashSet::new()
    };
    let replay: Vec<_> = state.sent.iter().chain(&state.pending).cloned().collect();
    if let Err(error) = foreign.apply(&replay) {
        log::warn!("sidecar local replay failed: {error}");
        return;
    }
    let records = state.records.clone();
    for tab in foreign.tabs() {
        state.baseline.insert(tab.id, tab.clone());
    }
    let windows = crate::ui::windows::WindowRegistry::open_windows(cx);
    for (workspace, app) in windows {
        if WorkspaceStore::remote_ref(cx, workspace).as_ref() != Some(&remote) {
            continue;
        }
        let (Some(app), Some(handle)) = (
            app.upgrade(),
            crate::ui::windows::WindowRegistry::window_for(cx, workspace),
        ) else {
            continue;
        };
        let effective = effective.clone();
        let _ = handle.update(cx, |_, window, cx| {
            app.update(cx, |app, cx| {
                adopt(app, &effective, &records, &closed_local, window, cx)
            });
        });
    }
}

fn adopt(
    app: &mut LyrelApp,
    sidecar: &Workspace,
    records: &HashMap<u64, lyrel_core::core::machine::PaneRecord>,
    closed_local: &HashSet<ScopedPane>,
    window: &mut Window,
    cx: &mut Context<LyrelApp>,
) {
    let Some(foreign) = &sidecar.foreign_view else {
        return;
    };
    let active = app.tabs.get(app.tabs.active_index()).map(|tab| tab.id());
    let mut slots: HashMap<ScopedPane, PaneSlot> = app
        .tabs
        .iter()
        .flat_map(|tab| tab.pane.leaves())
        .filter_map(|slot| scoped_pane(&slot, cx).map(|key| (key, slot)))
        .collect();
    let authoritative: HashSet<_> = foreign.tabs().iter().map(|tab| tab.id).collect();
    let focused = app
        .tabs
        .iter()
        .flat_map(|tab| tab.pane.leaves())
        .find(|slot| slot.contains_focused(window, cx))
        .map(|slot| slot.entity_id());
    app.tabs.retain(|tab| {
        tab.scoped_presentation().is_none() || authoritative.contains(&tab.id())
    });
    for source in foreign.tabs() {
        let pruned;
        let source = if closed_local.is_empty() {
            source
        } else {
            let Some(source_pruned) =
                source.map_panes(|pane| (!closed_local.contains(&pane)).then_some(pane))
            else {
                app.tabs.retain(|tab| tab.id() != source.id);
                continue;
            };
            pruned = source_pruned;
            &pruned
        };
        for key in source.root.pane_ids() {
            if slots.contains_key(&key) {
                continue;
            }
            // A missing view is not a removed leaf. Represent every surviving
            // authoritative ID through the existing attach-only pending flow so
            // save_session cannot publish a partial tree as a user deletion.
            let attached = if key.owner().is_client_local() {
                let spec = records
                    .get(&key.pane())
                    .and_then(|pane| pane.ssh_spec.clone());
                crate::ui::app::attach_sidecar_terminal(
                    key.pane(),
                    sidecar.id,
                    app.font_size,
                    spec,
                    window,
                    cx,
                )
            } else {
                crate::ui::app::attach_existing_terminal(
                    Some(
                        app.window_workspace(cx)
                            .expect("sidecar adoption requires its remote workspace route"),
                    ),
                    key.pane(),
                    app.workspace,
                    app.font_size,
                    None,
                    window,
                    cx,
                )
            };
            match attached {
                Ok(slot) => {
                    slots.insert(key, slot);
                }
                Err(error) => log::warn!("could not attach sidecar pane {}: {error:#}", key.pane()),
            }
        }
        let Some(pane) = pane_from_existing(&source.root, &mut slots) else {
            continue;
        };
        let mut presentation = source.presentation.clone();
        let existing = pane
            .leaves()
            .into_iter()
            .filter_map(|slot| scoped_pane(&slot, cx))
            .collect::<HashSet<_>>();
        presentation.remap_panes(|key| existing.contains(&key).then_some(key));
        if let Some(tab) = app.tabs.iter_mut().find(|tab| tab.id() == source.id) {
            // Preserve every existing view and focus owner; only composition is shared.
            tab.pane = pane;
            tab.adopt_scoped_presentation(presentation);
        } else {
            let mut tab = Tab::from_foreign(source.id, source.name.clone(), pane);
            tab.group.set(source.group);
            tab.adopt_scoped_presentation(presentation);
            app.tabs.push(tab);
        }
    }
    app.tabs.select(
        active
            .and_then(|active| app.tabs.iter().position(|tab| tab.id() == active))
            .unwrap_or_else(|| {
                app.tabs
                    .active_index()
                    .min(app.tabs.len().saturating_sub(1))
            }),
    );
    for tab in &mut app.tabs {
        tab.discard_detached_zoom();
    }
    if app.maximized.as_ref().is_some_and(|view| {
        !app.tabs.get(app.tabs.active_index()).is_some_and(|tab| {
            tab.pane
                .leaves()
                .iter()
                .any(|slot| slot.entity_id() == view.entity_id())
        })
    }) {
        app.maximized = None;
    }
    if focused.is_some_and(|focused| {
        !app.tabs
            .iter()
            .flat_map(|tab| tab.pane.leaves())
            .any(|slot| slot.entity_id() == focused)
    }) {
        app.tabs.select(
            app.tabs
                .active_index()
                .min(app.tabs.len().saturating_sub(1)),
        );
        app.focus_active(window, cx);
    }
    app.refocus_parked_pane(window, cx);
    app.save_session(cx);
    cx.notify();
}
