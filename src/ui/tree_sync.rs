pub(crate) mod desired;
pub(crate) mod projection;
mod reconcile;
mod lifecycle;

#[cfg(test)]
mod test_layout;

use desired::seeded_records;
use projection::{desired_tabs, logical_node, slot_pane_id};
use lifecycle::{ProjectionResult, WindowSync};
use reconcile::WsMirror;

use crate::ui::notification_history::NotificationWindowExt as _;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use gpui_kit::{App, Global};
use lyrel_core::core::machine::{
    Axis as TreeAxis, LayoutDelta, Machine, PaneNode, PaneRecord, Side, Tab as TreeTab, TabId,
    Workspace,
};
use lyrel_core::daemon::control::{ControlClient, ControlRequest, ReplyOk};
use lyrel_core::host::HostId;

use crate::core::group_key::WorkspaceGroups;
use crate::core::session::{Session, SessionPane, SessionTab, WorkspaceId, WorkspaceStore};
use crate::ui::app::LyrelApp;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::pane::{Pane, PaneSlot};

fn pane_from_existing(node: &PaneNode, slots: &mut HashMap<u64, PaneSlot>) -> Option<Pane> {
    match node {
        PaneNode::Leaf { pane } => slots.remove(pane).map(Pane::Leaf),
        PaneNode::Split { axis, ratio, a, b } => Some(Pane::split_node(
            match axis {
                TreeAxis::Horizontal => gpui_kit::Axis::Horizontal,
                TreeAxis::Vertical => gpui_kit::Axis::Vertical,
            },
            *ratio,
            pane_from_existing(a, slots)?,
            pane_from_existing(b, slots)?,
        )),
    }
}

/// Toggles presentation through the shared domain mutation. Every leaf, including
/// client-local native SSH, stays in the logical tree and keeps its existing view.
pub(crate) fn toggle_pip_for_tab(
    tab: &mut crate::ui::tab::Tab,
    pane: gpui_kit::EntityId,
    fallback: Option<gpui_kit::EntityId>,
    workspace: WorkspaceId,
    cx: &App,
) -> bool {
    change_presentation_for_tab(tab, pane, fallback, None, workspace, cx)
}

pub(crate) fn set_parked_for_tab(
    tab: &mut crate::ui::tab::Tab,
    pane: gpui_kit::EntityId,
    fallback: Option<gpui_kit::EntityId>,
    parked: bool,
    workspace: WorkspaceId,
    cx: &App,
) -> bool {
    change_presentation_for_tab(tab, pane, fallback, Some(parked), workspace, cx)
}

fn change_presentation_for_tab(
    tab: &mut crate::ui::tab::Tab,
    pane: gpui_kit::EntityId,
    fallback: Option<gpui_kit::EntityId>,
    parked: Option<bool>,
    workspace: WorkspaceId,
    cx: &App,
) -> bool {
    let leaves = tab.pane.leaves();
    let remote = WorkspaceStore::all(cx)
        .get(workspace)
        .is_some_and(|w| w.is_remote());
    if remote
        && let Some(changed) =
            crate::ui::pane_sidecar::change_for_tab(tab, pane, fallback, parked, cx)
    {
        return changed;
    }
    let Some(target_slot) = leaves.iter().find(|slot| slot.entity_id() == pane) else {
        return false;
    };
    if remote
        && matches!(target_slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
    {
        return false;
    }
    let Some(target) = slot_pane_id(target_slot, cx) else {
        return false;
    };
    let Some(root) = logical_node(&tab.pane, remote, cx) else {
        return false;
    };
    // An unassigned connecting leaf must not disappear when a restore reshapes.
    let mut slots = HashMap::with_capacity(leaves.len());
    let mut client_local = Vec::new();
    for slot in &leaves {
        if remote && matches!(slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
        {
            client_local.push(slot.clone());
            continue;
        }
        let Some(id) = slot_pane_id(slot, cx) else {
            return false;
        };
        if slots.insert(id, slot.clone()).is_some() {
            return false;
        }
    }
    let fallback = fallback
        .and_then(|id| leaves.iter().find(|slot| slot.entity_id() == id))
        .filter(|slot| {
            !remote
                || !matches!(slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
        })
        .and_then(|slot| slot_pane_id(slot, cx));
    let mut logical = TreeTab {
        root,
        presentation: tab.presentation.clone(),
        id: tab.id(),
        name: None,
        group: None,
        last_auto: None,
        hibernated: false,
    };
    let changed = match parked {
        Some(true) => logical.park(target),
        Some(false) => logical.restore_parked(target, fallback),
        None => {
            let promote = (logical.presentation.pip().map(|pip| pip.pane()) != Some(target))
                .then_some(target);
            logical.set_pip(promote, fallback)
        }
    };
    if !changed {
        return false;
    }
    if logical_node(&tab.pane, remote, cx).as_ref() != Some(&logical.root) {
        let Some(pane) = pane_from_existing(&logical.root, &mut slots) else {
            return false;
        };
        tab.pane = client_local.into_iter().fold(pane, |tree, slot| {
            Pane::split_node(gpui_kit::Axis::Horizontal, 0.5, tree, Pane::Leaf(slot))
        });
    }
    tab.presentation = logical.presentation;
    true
}

pub(crate) fn control_for(cx: &mut App, host: HostId) -> Option<Arc<ControlClient>> {
    if host.is_local() {
        crate::ui::local_link::LocalLink::client(cx)
    } else {
        crate::ui::remote_connect::HostLinks::get(cx, host)
            .map(|h| Arc::clone(h.client()))
            .filter(|c| c.is_connected())
    }
}

pub(crate) enum TreeLink {
    Ready(Arc<ControlClient>),
    Unserved,
    Down,
}

pub(crate) fn tree_control_for(cx: &mut App, host: HostId) -> TreeLink {
    classify_tree_link(control_for(cx, host))
}

fn classify_tree_link(client: Option<Arc<ControlClient>>) -> TreeLink {
    match client {
        Some(client)
            if client
                .hello()
                .has_feature(lyrel_core::daemon::control::feature::MACHINE_TREE) =>
        {
            TreeLink::Ready(client)
        }
        Some(_) => TreeLink::Unserved,
        None => TreeLink::Down,
    }
}

/// Whether the machine holding `client_ws` can put one of its tabs to sleep.
/// A machine that cannot would take the mark and leave the shells running, or
/// refuse it outright, so a window never offers to on its behalf.
pub(crate) fn can_hibernate_on(cx: &App, client_ws: WorkspaceId) -> bool {
    let feature = lyrel_core::daemon::control::feature::TAB_HIBERNATE;
    let host = WorkspaceStore::host_of(cx, client_ws);
    match host.is_local() {
        true => crate::ui::local_link::LocalLink::supports(cx, feature),
        false => crate::ui::remote_connect::HostLinks::peer_supports(cx, host, feature),
    }
}

/// Whether the machine holding `client_ws` keeps its recently closed tabs
/// (#1021). A machine that does not — an older daemon, a remote server not yet
/// updated — is closed and reopened against the way it always was, from the
/// window's own list.
pub(crate) fn keeps_closed_tabs_on(cx: &App, client_ws: WorkspaceId) -> bool {
    let feature = lyrel_core::daemon::control::feature::CLOSED_TABS;
    let host = WorkspaceStore::host_of(cx, client_ws);
    match host.is_local() {
        true => crate::ui::local_link::LocalLink::supports(cx, feature),
        false => crate::ui::remote_connect::HostLinks::peer_supports(cx, host, feature),
    }
}

/// Whether closing `tab` can go to the machine's recently-closed list: the
/// machine keeps one, and this window's next sync is certain to tell it about
/// the close — it has pulled its layout, speaks for every tab in it, and the
/// machine has the tab. Anything short of that and the close would never
/// reach the machine, so the window keeps the tab to reopen itself.
pub(crate) fn remembers_closed_tab(cx: &App, client_ws: WorkspaceId, tab: TabId) -> bool {
    if !keeps_closed_tabs_on(cx, client_ws) {
        return false;
    }
    let Some(state) = cx
        .try_global::<TreeSync>()
        .and_then(|t| t.windows.get(&client_ws))
    else {
        return false;
    };
    state.remembers_close(tab)
}

/// A tab this window closed into its machine's recently-closed list.
///
/// Its panes are left for the machine to stop, because the machine is what
/// keeps their screens for the reopen — a kill from here drops them. Until the
/// machine has said it did, this is also the window's note to end them itself
/// if the close never gets there.
#[derive(Clone, Debug)]
pub(crate) struct Retiring {
    pub tab: TabId,
    /// The tab's panes on the window's machine.
    pub panes: Vec<u64>,
    pub route: crate::terminal::PaneRoute,
}

/// Records that the user closed `tab` in this window, which is what lets the
/// next sync close it on the machine.
///
/// A sync never reads a tab's absence from the window as a close. A window can
/// be missing tabs for many reasons that are not the user's — emptied while a
/// restart hands the server over, a layout that would not rebuild, state left
/// from an earlier visit — and every one of those, read as a close, deleted
/// tabs whose shells kept running with nothing left to hold them (#554, #579,
/// #672, #716, and Restart Server emptying a workspace with sixteen agents in
/// it). Absence alone now only ever leaves the machine's tab where it is.
pub(crate) fn intend_close(cx: &mut App, client_ws: WorkspaceId, tab: TabId) {
    crate::ui::pane_sidecar::intend_close(cx, client_ws, tab);
    cx.default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default()
        .intend_close(tab);
}

/// Hand a closing tab to the next sync, which tells the machine to remember it
/// rather than forget it. See [`take_unsent_retirement`] for the other half.
pub(crate) fn retire_tab(cx: &mut App, client_ws: WorkspaceId, retiring: Retiring) {
    cx.default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default()
        .retire(retiring);
}

/// After the sync a close ran: the retirement of `tab` if that sync did not
/// pass it on to the machine, withdrawn, for the caller to close the tab the
/// old way instead. `None` means the machine has it.
pub(crate) fn take_unsent_retirement(
    cx: &mut App,
    client_ws: WorkspaceId,
    tab: TabId,
) -> Option<Retiring> {
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .get_mut(&client_ws)?;
    state.withdraw_retirement(tab)
}

/// Whether everything this window has told its machine has been answered.
///
/// ⌘⇧T asks the machine for the newest closed tab, and the close it means to
/// undo may still be queued behind other edits: asking first would reopen the
/// one before it.
pub(crate) fn tree_ops_settled(cx: &App, client_ws: WorkspaceId) -> bool {
    cx.try_global::<TreeSync>()
        .and_then(|t| t.windows.get(&client_ws))
        .is_none_or(WindowSync::is_settled)
}

/// The link to ask for a closed tab back on, and the machine's id for the
/// workspace — `None` when that machine keeps no list or cannot be reached.
pub(crate) fn closed_tabs_link(
    cx: &mut App,
    client_ws: WorkspaceId,
) -> Option<(Arc<ControlClient>, WorkspaceId)> {
    if !keeps_closed_tabs_on(cx, client_ws) {
        return None;
    }
    let host = WorkspaceStore::host_of(cx, client_ws);
    match tree_control_for(cx, host) {
        TreeLink::Ready(client) => Some((client, tree_workspace_id(cx, client_ws))),
        TreeLink::Unserved | TreeLink::Down => None,
    }
}

/// The tab a reopen answered, as the restore reads a tab: the same layout,
/// each leaf carrying its old pane id for the new pane to open on the screen
/// of, and the cwd, shell, SSH target and agent facts the machine kept for it.
/// The tab keeps its id, so the machine hears it come back as itself.
pub(crate) fn session_tab_from_reopened(
    reopened: &lyrel_core::core::machine::ReopenedTab,
) -> SessionTab {
    closed_session_tab(&reopened.tab, &reopened.panes)
}

fn closed_session_tab(tab: &TreeTab, panes: &[PaneRecord]) -> SessionTab {
    SessionTab {
        name: tab.name.clone(),
        pane: session_pane_from_node(&tab.root, panes),
        group: tab.group,
        last_auto: tab.last_auto.clone(),
        tree_id: Some(tab.id),
        presentation: tab.presentation.clone(),
        hibernated: false,
        asleep_view: None,
    }
}

/// The newest closed tab the machine keeps for `client_ws`, as this client's
/// mirror of the machine knows it — what the home screen offers back. One
/// past its day is not offered, whatever the mirror still holds: the machine
/// has let it go, or is about to.
pub(crate) fn newest_remembered_close(cx: &App, client_ws: WorkspaceId) -> Option<SessionTab> {
    if !keeps_closed_tabs_on(cx, client_ws) {
        return None;
    }
    let host = WorkspaceStore::host_of(cx, client_ws);
    let machine = crate::ui::machine_mirror::MachineMirrors::machine(cx, host)?;
    let machine_ws = tree_workspace_id(cx, client_ws);
    let ws = machine.workspaces.iter().find(|w| w.id == machine_ws)?;
    let now = crate::ui::home::now_secs();
    let ttl = lyrel_core::core::machine::CLOSED_TAB_TTL.as_secs();
    let entry = ws
        .closed
        .iter()
        .rev()
        .find(|c| now.saturating_sub(c.closed_at) < ttl)?;
    Some(closed_session_tab(&entry.tab, &machine.panes))
}


/// End, from here, the panes of every remembered close in `dropped` — ops the
/// machine was sent and refused, or that were never sent at all. Nobody else
/// is going to stop those panes now, and a close that leaves its shells
/// running is worse than one that loses their screens.
fn end_unsent_retirements(cx: &mut App, retired: &mut Vec<Retiring>, dropped: &[ControlRequest]) {
    for op in dropped {
        let ControlRequest::TabCloseRemembered { tab, .. } = op else {
            continue;
        };
        let Some(at) = retired.iter().position(|r| r.tab == *tab) else {
            continue;
        };
        let Retiring { panes, route, .. } = retired.remove(at);
        log::info!("the machine never took closed tab {tab}; ending its panes from here");
        cx.background_executor()
            .spawn(async move {
                for pane in panes {
                    crate::terminal::RemoteTerminal::kill_pane_on(&route, pane);
                }
            })
            .detach();
    }
}

fn tree_workspace_id(cx: &App, client_ws: WorkspaceId) -> WorkspaceId {
    WorkspaceStore::all(cx)
        .get(client_ws)
        .and_then(|w| w.host.as_ref())
        .map(|r| r.workspace)
        .unwrap_or(client_ws)
}



/// A name the user typed, and the workspace they typed it for.
///
/// The workspace id is the whole point. Parked on its own, a name is just "the
/// last thing somebody typed into a create form", and `settle_chosen_name` had
/// no way to tell a workspace it had named into existence from one the window
/// had since walked into — so it renamed whichever workspace the window
/// happened to land on (#716). Carrying the id it was chosen for makes that
/// question answerable.
#[derive(Clone, Debug)]
struct ChosenName {
    /// The workspace on the machine — `tree_workspace_id`, not the client's own
    /// id — that this name was typed for.
    workspace: WorkspaceId,
    name: String,
}

/// How the workspace a pull answered for got there.
///
/// The name a user types belongs to a create. When one runs — this client's, or
/// a create of its own that raced it and won with a rolled codename — the typed
/// name is owed and goes out as a rename if the machine came back with anything
/// else (#618, #604). When the workspace was simply already on the machine,
/// nothing was created, the window walked into somebody else's workspace, and
/// the name it is called is its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arrival {
    /// A create ran for this workspace as part of this pull.
    Created,
    /// The workspace was on the machine before this window asked for it.
    Adopted,
}


#[derive(Default)]
pub(crate) struct TreeSync {
    windows: HashMap<WorkspaceId, WindowSync>,
}

impl Global for TreeSync {}

/// Publishes a saved local layout; reconnect/retry coordination is not a user edit.
pub(crate) fn sync_window(app: &LyrelApp, cx: &mut App) {
    if let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&app.workspace) {
        state.record_local_write();
    }
    coordinate_window(app, cx);
}

fn coordinate_window(app: &LyrelApp, cx: &mut App) {
    let client_ws = app.workspace;
    if !cx.has_global::<crate::core::session::WorkspaceStore>() {
        return;
    }
    crate::ui::pane_sidecar::sync_window(app, cx);
    if crate::ui::remote_workspace::workspace_is_preempted(cx, client_ws) {
        return;
    }
    let showing: Vec<TabId> = app.tabs.iter().map(|t| t.id()).collect();
    if let Some(adopt) = take_rehydrate(cx, client_ws, &showing) {
        // The app is mid-update here, so the tabs it knows it is showing come
        // from `app` itself, not from reading the entity back.
        hydrate_with(cx, client_ws, adopt, showing);
        return;
    }
    adopt_tab_ids(app, cx);
    let (desired, desired_active, held) = desired_tabs(&app.tabs, app.tabs.active_index(), client_ws, cx);
    let machine_ws = tree_workspace_id(cx, client_ws);

    let state = cx
        .default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default();
    match state.project(client_ws, machine_ws, &desired, desired_active, held) {
        ProjectionResult::Prime => start_prime(cx, client_ws),
        ProjectionResult::Waiting => {}
        ProjectionResult::Queued { tabs, active, remembered } => {
            let host = WorkspaceStore::host_of(cx, client_ws);
            // Remember closes before replacing the machine mirror's open tabs.
            crate::ui::machine_mirror::MachineMirrors::note_remembered_closes(
                cx, host, machine_ws, remembered,
            );
            crate::ui::machine_mirror::MachineMirrors::note_synced_workspace(
                cx, host, machine_ws, tabs, active,
            );
            let open: Vec<u64> = app.tabs.iter()
                .flat_map(|t| t.pane.terminals())
                .map(|v| v.read(cx).pane_id)
                .collect();
            crate::ui::machine_mirror::MachineMirrors::note_seeded_panes(
                cx, host, seeded_records(&desired, |pane| open.contains(&pane)),
            );
            pump(cx, client_ws);
        }
    }
}

pub(crate) fn on_link_up(cx: &mut App, host: HostId) {
    if host.is_local() {
        crate::ui::pane_sidecar::on_local_link_up(cx);
    }
    for (workspace, app) in crate::ui::windows::WindowRegistry::open_windows(cx) {
        if WorkspaceStore::host_of(cx, workspace) != host {
            continue;
        }
        if let Some(app) = app.upgrade() {
            app.update(cx, |app, cx| coordinate_window(app, cx));
        }
    }
}

/// Claims a hydration owed to `client_ws`, if one is still outstanding.
/// `showing` is what the window has on screen right now.
///
/// A `Replace` retry is dropped once the user has filled the window in without
/// us: replaying the machine's older layout over work they did in the meantime
/// would be worse than never retrying at all.
///
/// What counts as "without us" is a tab the debt never saw. Judging it by the
/// window merely *having* tabs read every resync of a window that kept its own
/// — a daemon back as a new process, a delta that would not apply, a remote
/// server restarted — as the user having moved on, when those tabs are the
/// stale ones the `Replace` was ordered to sweep away. The retry was abandoned
/// on its first attempt, every time, and the resync silently did nothing.
fn take_rehydrate(cx: &mut App, client_ws: WorkspaceId, showing: &[TabId]) -> Option<Adopt> {
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .get_mut(&client_ws)?;
    state.take_rehydrate(showing)
}

/// Whether a window with no tabs may delete `client_ws` outright — from the
/// machine's tree and from the store both.
///
/// Two independent things have to agree, because the window's own emptiness
/// cannot tell them apart: a workspace is empty when it genuinely holds
/// nothing, and equally when its layout failed to rebuild. Only the first is a
/// reason to delete anything, and the second has already cost a workspace with
/// ten live tabs in it.
///
/// So the window must be informed (it pulled a layout and put it up), *and* the
/// mirror — the machine's own account, which no local failure can empty — must
/// agree there is nothing there. An unprimed mirror knows nothing and answers
/// no: "I don't know" may never authorize a deletion.
pub(crate) fn workspace_is_disposable(cx: &App, client_ws: WorkspaceId) -> bool {
    let Some(state) = cx
        .try_global::<TreeSync>()
        .and_then(|t| t.windows.get(&client_ws))
    else {
        return false;
    };
    state.is_disposable()
}

pub(crate) fn mark_window_informed(cx: &mut App, client_ws: WorkspaceId) {
    cx.default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default()
        .inform();
}

fn adopt_tab_ids(app: &LyrelApp, cx: &App) {
    let Some(state) = cx.try_global::<TreeSync>().and_then(|t| t.windows.get(&app.workspace)) else {
        return;
    };
    state.adopt_tab_ids(&app.tabs, cx);
}

pub(crate) fn on_preempted(cx: &mut App, client_ws: WorkspaceId) {
    clear_queue(cx, client_ws);
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return;
    };
    state.preempt();
}

pub(crate) fn forget(cx: &mut App, client_ws: WorkspaceId) {
    if cx.try_global::<TreeSync>().is_some() {
        // What is still queued goes nowhere once the window is gone. A batch
        // already in flight finishes on its own, and the machine answers it.
        clear_queue(cx, client_ws);
        cx.default_global::<TreeSync>().windows.remove(&client_ws);
    }
}

/// Throw away whatever this window still had queued for its machine, ending
/// from here the panes of any remembered close among it.
fn clear_queue(cx: &mut App, client_ws: WorkspaceId) {
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return;
    };
    let (dropped, mut retired) = state.discard_queue();
    end_unsent_retirements(cx, &mut retired, &dropped);
}

pub(crate) fn fire_workspace_op(
    cx: &mut App,
    client_ws: WorkspaceId,
    op: impl FnOnce(WorkspaceId) -> ControlRequest,
) {
    if !cx.has_global::<crate::core::session::WorkspaceStore>() {
        return;
    }
    let host = WorkspaceStore::host_of(cx, client_ws);
    let machine_ws = tree_workspace_id(cx, client_ws);
    let request = op(machine_ws);
    crate::ui::machine_mirror::MachineMirrors::note_workspace_op(cx, host, &request);
    let client = match tree_control_for(cx, host) {
        TreeLink::Ready(client) => client,
        TreeLink::Unserved => {
            unsendable(
                &request,
                "this machine's server does not serve the workspace tree",
            );
            return;
        }
        TreeLink::Down => {
            unsendable(&request, "there is no control link to its machine");
            return;
        }
    };
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = client.call(request.clone()) {
                unsendable(&request, &format!("the machine refused it: {e}"));
            }
        })
        .detach();
}

fn unsendable(request: &ControlRequest, why: &str) {
    match request {
        ControlRequest::WorkspaceRemove { workspace } => log::warn!(
            "workspace {workspace} was deleted here but not on its machine ({why}); \
             its entry stays in that machine's tree, where another client will still \
             see it — delete it again from a client that can reach the machine"
        ),
        other => log::debug!("{other:?} not sent ({why}); the next edit carries it"),
    }
}

/// Names a workspace this window is in the middle of creating.
///
/// Parked rather than sent: the machine has not been asked to create the
/// workspace yet, so a rename addressed to it right now comes back `NotFound`
/// and is dropped on the floor — which is how a name typed into the create form
/// used to lose to the generated one (#618). `start_prime` spends it on the
/// create itself.
///
/// A window already synced with its machine has no create coming, so there is
/// nothing to ride along with and the rename goes out as usual.
pub(crate) fn name_new_workspace(cx: &mut App, client_ws: WorkspaceId, name: String) {
    // Read before the state is borrowed, and read *now* rather than at settle
    // time: this is the moment the user named a particular workspace, and it is
    // the only moment at which the pairing is certain.
    let workspace = tree_workspace_id(cx, client_ws);
    // A window tree-sync has never heard of is as unprimed as one it is
    // priming right now: either way the create is still ahead of us.
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default();
    if let Err(name) = state.park_name(workspace, name) {
        rename_workspace(cx, client_ws, Some(name));
    }
}

/// Puts `client_ws` in the state a window is in once its pull has landed:
/// primed on `tabs` and speaking for the machine in full. Marked in flight, so
/// what a sync queues stays queued for [`queued_for_test`] to read.
#[cfg(test)]
pub(crate) fn prime_for_test(cx: &mut App, client_ws: WorkspaceId, tabs: Vec<TreeTab>) {
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default();
    state.prime_for_test(tabs);
}

#[cfg(test)]
pub(crate) fn queued_for_test(cx: &mut App, client_ws: WorkspaceId) -> Vec<ControlRequest> {
    cx.default_global::<TreeSync>()
        .windows
        .get(&client_ws)
        .map(WindowSync::queued_for_test)
        .unwrap_or_default()
}

/// The name parked for a create that has not run yet, for tests that need to
/// see it got that far.
#[cfg(test)]
pub(crate) fn chosen_name_for(cx: &mut App, client_ws: WorkspaceId) -> Option<String> {
    cx.default_global::<TreeSync>()
        .windows
        .get(&client_ws)
        .and_then(WindowSync::chosen_name_for_test)
}

pub(crate) fn rename_workspace(cx: &mut App, client_ws: WorkspaceId, name: Option<String>) {
    fire_workspace_op(cx, client_ws, move |ws| ControlRequest::WorkspaceRename {
        workspace: ws,
        name,
    });
}

fn start_prime(cx: &mut App, client_ws: WorkspaceId) {
    let host = WorkspaceStore::host_of(cx, client_ws);
    let machine_ws = tree_workspace_id(cx, client_ws);
    let client = match tree_control_for(cx, host) {
        TreeLink::Ready(client) => client,
        unavailable => {
            if matches!(unavailable, TreeLink::Unserved) {
                log::warn!(
                    "workspace {client_ws}: its machine's server does not serve the tree; \
                     the layout will not be synced"
                );
            }
            if let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) {
                state.prime_unavailable();
            }
            return;
        }
    };
    let epoch = cx
        .default_global::<TreeSync>()
        .windows
        .get(&client_ws)
        .map(WindowSync::epoch)
        .unwrap_or(0);
    cx.spawn(async move |cx| {
        // Picked on the main thread, where the names already in use are
        // readable, but inside the task rather than before it: a window that is
        // switching workspaces orders its pull first and is named second, so
        // reading any earlier would miss the name the user typed. A name the
        // user did type beats a generated one, and is left in place for
        // `finish_prime` to spend against the machine's answer.
        let fresh = cx.update(|cx| {
            cx.default_global::<TreeSync>()
                .windows
                .get(&client_ws)
                .and_then(|state| state.chosen_name(machine_ws))
                .unwrap_or_else(|| fresh_workspace_name(cx, host))
        });
        let outcome = cx
            .background_executor()
            .spawn(async move { pull_or_create(&client, machine_ws, fresh) })
            .await;
        cx.update(|cx| finish_prime(cx, client_ws, epoch, outcome));
    })
    .detach();
}

/// A codename no workspace on `host` is using. Beats leaving new workspaces
/// named after whatever directory their first shell happened to start in —
/// three of those in a switcher all read the same.
pub(crate) fn fresh_workspace_name(cx: &App, host: HostId) -> String {
    let mut taken: Vec<String> = Vec::new();
    if let Some(machine) = crate::ui::machine_mirror::MachineMirrors::machine(cx, host) {
        taken.extend(machine.user_workspaces().filter_map(|w| w.name.clone()));
    }
    // Labels are the names the switcher actually shows, which for an unnamed
    // workspace is its directory. Counting those as taken is deliberately
    // generous — it only ever costs another roll of the dice.
    if cx.has_global::<WorkspaceStore>() {
        taken.extend(
            WorkspaceStore::all(cx)
                .views
                .iter()
                .filter(|w| w.host_id() == host)
                .filter_map(|w| w.label.clone()),
        );
    }
    lyrel_core::core::codename::unique(|name| taken.iter().any(|t| t == name))
}

/// This workspace's layout, and the name the machine has for it.
///
/// The name comes back even from the create this client asked for, and
/// especially from that one: a client is left out of the deltas its own ops
/// raise, so the `WorkspaceCreated` delta carrying the name it just proposed
/// never arrives. Dropping the name here left the mirror holding the workspace
/// unnamed until something pulled the whole tree again, and the name it had had
/// all along then landed on screen looking like a rename (#604).
/// Settles the name a window asked for against the name its machine came back
/// with, and returns what the workspace is really called.
///
/// `answered` is the machine's answer. A chosen name it read back was spent by
/// the create that carried it, and there is nothing left to do. One it did not
/// means the create ran under a different name — this client's create lost the
/// race to a create of its own that had rolled a codename before the user had
/// finished typing — so it goes out as the rename it has become (#618, #604).
/// Either way the name is owed only once.
///
/// `arrival` is what stops that override from firing at a workspace nobody
/// asked to create. A name is spent only on a create it actually rode along
/// with: the workspace it was typed for, and a pull that had to make that
/// workspace. Walking into a workspace the machine already had renamed it to
/// whatever the arriving client had parked — which is how a remote client's
/// login name ended up on somebody else's workspace (#716).
///
/// A name that does not match this arrival is left parked rather than dropped.
/// Two pulls run at once when a window opens a workspace (`start_prime`'s and
/// `hydrate`'s) and only one of them creates; taking the name on the adopting
/// one would let the loser of that race swallow it before the winner could
/// spend it, which is the #618 regression this is trying not to reintroduce.
/// A parked name outlives nothing: `forget` drops the whole state when the
/// window leaves the workspace.
fn settle_chosen_name(
    cx: &mut App,
    client_ws: WorkspaceId,
    answered: Option<String>,
    arrival: Arrival,
) -> Option<String> {
    let machine_ws = tree_workspace_id(cx, client_ws);
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return answered;
    };
    let Some(chosen) = state.settle_name(machine_ws, arrival) else {
        return answered;
    };
    if answered.as_deref() == Some(chosen.as_str()) {
        return answered;
    }
    rename_workspace(cx, client_ws, Some(chosen.clone()));
    Some(chosen)
}

fn pull_or_create(
    client: &ControlClient,
    machine_ws: WorkspaceId,
    fresh: String,
) -> io::Result<(WsMirror, Option<String>, Arrival)> {
    match client.call(ControlRequest::WorkspaceTree {
        workspace: machine_ws,
    }) {
        // The tree answered, so nothing was created here — this window walked
        // into a workspace that was already on the machine.
        Ok(ReplyOk::WorkspaceTree(ws)) => Ok(primed(*ws, Arrival::Adopted)),
        Ok(other) => Err(io::Error::other(format!(
            "WorkspaceTree answered {other:?}"
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match client.call(ControlRequest::WorkspaceCreate {
                name: Some(fresh),
                workspace: Some(machine_ws),
            })? {
                ReplyOk::WorkspaceTree(ws) => Ok(primed(*ws, Arrival::Created)),
                other => Err(io::Error::other(format!(
                    "WorkspaceCreate answered {other:?}"
                ))),
            }
        }
        Err(e) => Err(e),
    }
}

fn primed(ws: Workspace, arrival: Arrival) -> (WsMirror, Option<String>, Arrival) {
    (
        WsMirror {
            tabs: ws.tabs,
            active: ws.active_tab,
            groups: ws.groups,
        },
        ws.name,
        arrival,
    )
}

fn finish_prime(
    cx: &mut App,
    client_ws: WorkspaceId,
    epoch: u64,
    outcome: io::Result<(WsMirror, Option<String>, Arrival)>,
) {
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return;
    };
    if !state.accepts_prime(epoch) {
        log::debug!("workspace {client_ws}: dropping a superseded tree pull");
        return;
    }
    let (mirror, name, arrival) = match outcome {
        Ok(pulled) => pulled,
        Err(e) => {
            log::warn!("could not pull the tree for workspace {client_ws}: {e}");
            state.prime_failed();
            return;
        }
    };
    let landed = state.prime_landed(mirror);
    adopt_groups(cx, client_ws);
    let host = WorkspaceStore::host_of(cx, client_ws);
    let machine_ws = tree_workspace_id(cx, client_ws);
    crate::ui::machine_mirror::MachineMirrors::note_synced_workspace(
        cx, host, machine_ws, landed.tabs, landed.active,
    );
    // The pull above is the only place this window will hear the workspace's
    // name — it is left out of the deltas its own create raises (#604).
    let name = settle_chosen_name(cx, client_ws, name, arrival);
    crate::ui::machine_mirror::MachineMirrors::note_workspace_name(cx, host, machine_ws, name);
    if !landed.dirty {
        return;
    }
    let Some(app) =
        crate::ui::windows::WindowRegistry::app_for(cx, client_ws).and_then(|app| app.upgrade())
    else {
        return;
    };
    app.update(cx, |app, cx| coordinate_window(app, cx));
}

/// Sends a window's edit to its workspace's sidebar groups up to the machine.
///
/// Not part of the diff [`sync_window`] runs, on purpose. Groups have nothing
/// a window can know better than the machine: a fresh window holds none at
/// all, and diffing that emptiness against the tree would unpin every group
/// the workspace had. So they only ever go up as what they are — an edit
/// someone just made — and come down from every pull and every
/// `GroupsChanged`.
///
/// Queued with the tab ops rather than fired beside them, and ahead of the
/// ones the same edit raises (the caller pushes this before it saves): a tab
/// filed into a group that was just made must not reach the machine before
/// the group does, or `workspace_set_groups`, which hands tabs naming unknown
/// groups back to auto grouping, would see a tab pointing at nothing.
///
/// A window whose pull has not landed parks the edit, and the pull sends it
/// on instead of overwriting it — otherwise a pin made in the first moments
/// of a window's life would be undone by the tree that arrives after it.
pub(crate) fn push_groups(cx: &mut App, client_ws: WorkspaceId, groups: WorkspaceGroups) {
    if !cx.has_global::<crate::core::session::WorkspaceStore>() {
        return;
    }
    let machine_ws = tree_workspace_id(cx, client_ws);
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default();
    if state.push_groups(machine_ws, groups) {
        pump(cx, client_ws);
    }
}

/// Hands the window the sidebar groups a pull just brought in — or, when the
/// window edited them before the pull landed, sends that edit up instead.
///
/// Deferred: a pull can land while the window is itself mid-update (its own
/// `sync_window` started it), and the window is only updated once that is
/// over. Nothing in between can push the window's stale copy, because groups
/// only ever go up from an edit — see [`push_groups`].
fn adopt_groups(cx: &mut App, client_ws: WorkspaceId) {
    let machine_ws = tree_workspace_id(cx, client_ws);
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return;
    };
    if !state.adopt_groups_from_pull(machine_ws) {
        pump(cx, client_ws);
        return;
    }
    cx.defer(move |cx| {
        let Some(groups) = cx.default_global::<TreeSync>()
            .windows.get(&client_ws).and_then(WindowSync::groups)
        else {
            return;
        };
        if !cx.has_global::<crate::ui::windows::WindowRegistry>() {
            return;
        }
        let Some(app) =
            crate::ui::windows::WindowRegistry::app_for(cx, client_ws).and_then(|a| a.upgrade())
        else {
            return;
        };
        app.update(cx, |app, cx| app.adopt_sidebar_groups(groups, cx));
    });
}

fn pump(cx: &mut App, client_ws: WorkspaceId) {
    let host = WorkspaceStore::host_of(cx, client_ws);
    let client = tree_control_for(cx, host);
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default();
    if !state.has_batch_ready() {
        return;
    }
    let client = match client {
        TreeLink::Ready(client) => client,
        TreeLink::Unserved => {
            desync(cx, client_ws, "the server does not serve the machine tree");
            return;
        }
        TreeLink::Down => {
            desync(cx, client_ws, "the control link is down");
            return;
        }
    };
    let lifecycle::Batch { epoch, ops: batch, retired } = state.begin_batch().expect("batch ready");
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move {
                let mut ops = batch.into_iter();
                while let Some(op) = ops.next() {
                    if let Err(e) = client.call(op.clone()) {
                        let unsent: Vec<ControlRequest> =
                            std::iter::once(op.clone()).chain(ops).collect();
                        return Err((op, e, unsent));
                    }
                }
                Ok(())
            })
            .await;
        cx.update(|cx| apply_batch_result(cx, client_ws, epoch, retired, result));
    })
    .detach();
}

type BatchOutcome = Result<(), (ControlRequest, io::Error, Vec<ControlRequest>)>;

fn apply_batch_result(
    cx: &mut App,
    client_ws: WorkspaceId,
    epoch: u64,
    mut retired: Vec<Retiring>,
    result: BatchOutcome,
) {
    // The issued batch owns its closes, including when the window or daemon
    // generation changed while its calls were in flight.
    if let Err((_, _, unsent)) = &result {
        end_unsent_retirements(cx, &mut retired, unsent);
    }
    let current = cx.default_global::<TreeSync>().windows.get_mut(&client_ws)
        .is_some_and(|state| state.finish_batch(epoch));
    if !current {
        return;
    }
    match result {
        Ok(()) => pump(cx, client_ws),
        Err((op, e, _)) => {
            log::warn!("tree operation {op:?} failed: {e}; re-pulling the tree");
            desync(cx, client_ws, "an operation was refused");
        }
    }
}

fn desync(cx: &mut App, client_ws: WorkspaceId, why: &str) {
    log::info!("resynchronizing workspace {client_ws} with its machine ({why})");
    clear_queue(cx, client_ws);
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return;
    };
    state.reprime();
    start_prime(cx, client_ws);
}

pub(crate) fn session_from_tree(
    ws: &lyrel_core::core::machine::Workspace,
    panes: &[PaneRecord],
) -> Session {
    let views = lyrel_core::core::tab_view::tab_views_of(ws, panes);
    let tabs: Vec<SessionTab> = ws
        .tabs
        .iter()
        .zip(views)
        .map(|(tab, view)| SessionTab {
            name: tab.name.clone(),
            tree_id: Some(tab.id),
            group: tab.group,
            last_auto: tab.last_auto.clone(),
            pane: session_pane_from_node(&tab.root, panes),
            presentation: tab.presentation.clone(),
            hibernated: tab.hibernated,
            asleep_view: tab.hibernated.then_some(view),
        })
        .collect();
    let active = ws
        .active_tab
        .and_then(|id| ws.tabs.iter().position(|t| t.id == id))
        .unwrap_or(0);
    Session { active, tabs }
}

fn session_pane_from_node(node: &PaneNode, panes: &[PaneRecord]) -> SessionPane {
    match node {
        PaneNode::Leaf { pane } => {
            let record = panes.iter().find(|p| p.id == *pane);
            let (cwd, ssh_spec, agent, shell) = match record {
                Some(r) => (
                    r.cwd.clone().map(std::path::PathBuf::from),
                    r.ssh_spec.clone(),
                    r.agent.clone(),
                    r.shell.clone(),
                ),
                None => (None, None, None, None),
            };
            SessionPane::Leaf {
                cwd,
                // The id goes down whatever `live` says. That flag is a cached
                // fact about another process, written by whoever last observed
                // the pane and reloaded from disk as `false` on every server
                // start — so a quiet pane that nobody has observed since reads
                // as dead while its shell is very much alive. Believing it here
                // is what threw away live sessions on a workspace switch: the
                // id was erased, and the restore below had nothing to attach
                // to, so it spawned a fresh shell over a running one.
                //
                // Attaching is the thing that actually knows. `spawn_shell_
                // terminal_in` attaches when the pane is there and spawns fresh
                // when it is not, which is the same answer this filter was
                // trying to guess — except it is right. `live` stays a hint for
                // what to show, never the judge of what to destroy.
                pane_id: Some(*pane),
                shell,
                ssh_spec,
                agent: agent.as_ref().map(|a| a.agent),
                agent_session_id: agent.as_ref().and_then(|a| a.session_id.clone()),
                agent_launch_argv: agent.as_ref().and_then(|a| a.launch_argv.clone()),
                agent_session_scope: agent
                    .as_ref()
                    .and_then(|a| a.session_scope.as_ref())
                    .map(lyrel_core::host::session_operations::durable_scope),
            }
        }
        PaneNode::Split { axis, ratio, a, b } => SessionPane::Split {
            axis: match axis {
                TreeAxis::Horizontal => crate::core::session::SessionAxis::Horizontal,
                TreeAxis::Vertical => crate::core::session::SessionAxis::Vertical,
            },
            ratio: *ratio,
            a: Box::new(session_pane_from_node(a, panes)),
            b: Box::new(session_pane_from_node(b, panes)),
        },
    }
}

const HYDRATE_LINK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
const HYDRATE_LINK_POLL: std::time::Duration = std::time::Duration::from_millis(200);

pub(crate) fn hydrate_window_from_tree(cx: &mut App, client_ws: WorkspaceId) {
    hydrate(cx, client_ws, Adopt::IfEmpty);
}

/// Hydrates a window whose on-screen tabs are already known to the caller.
///
/// Reading them back with `tabs_on_screen` is illegal while the app's own
/// update lease is held — gpui aborts on a read of an entity that is already
/// being updated — and `switch_workspace` runs inside exactly that lease. The
/// ids are the same ones the read would produce: the tabs the window shows
/// right now.
pub(crate) fn hydrate_window_with_tabs(cx: &mut App, client_ws: WorkspaceId, showing: Vec<TabId>) {
    hydrate_with(cx, client_ws, Adopt::IfEmpty, showing);
}

/// Pulls this window's layout, then opens `path` as one more tab in it.
///
/// This is what a launch carrying a directory does — Explorer's "Open in lyrel",
/// or `lyrel <PATH>` with no window up. Both halves are wanted: the layout the
/// user left, and the folder they just double-clicked.
pub(crate) fn hydrate_window_then_open(
    cx: &mut App,
    client_ws: WorkspaceId,
    path: std::path::PathBuf,
) {
    cx.default_global::<TreeSync>()
        .windows
        .entry(client_ws)
        .or_default()
        .park_open(ParkedOpen::Folder(path));
    hydrate(cx, client_ws, Adopt::IfEmpty);
}

/// Parks a tab request while this window's layout is still on its way, and
/// says whether it did. A caller told `false` has a settled window and should
/// open the tab itself; one told `true` has handed the request to the pull,
/// which opens it once the layout is up.
///
/// Inserting into a window mid-pull is the failure `then_open` exists to
/// avoid: `Adopt::IfEmpty` declines a window that has a tab, and pushes that
/// one tab back as the whole workspace.
pub(crate) fn park_command_while_pulling(
    cx: &mut App,
    client_ws: WorkspaceId,
    cwd: &std::path::Path,
    command: &str,
) -> bool {
    if !layout_is_pending(cx, client_ws) {
        return false;
    }
    cx.default_global::<TreeSync>().windows.entry(client_ws).or_default()
        .park_open(ParkedOpen::Command { cwd: cwd.to_path_buf(), command: command.to_owned() });
    true
}

/// [`park_command_while_pulling`] for an `ssh://` link, which opens a tab of
/// its own and so races the same pull.
pub(crate) fn park_ssh_while_pulling(
    cx: &mut App,
    client_ws: WorkspaceId,
    ssh: &lyrel_core::core::ssh_profile::QuickConnect,
) -> bool {
    if !layout_is_pending(cx, client_ws) {
        return false;
    }
    cx.default_global::<TreeSync>().windows.entry(client_ws).or_default()
        .park_open(ParkedOpen::Ssh(ssh.clone()));
    true
}

/// Whether this window's layout is still on its way: a pull is out, one is
/// owed a retry, or tabs are already waiting on one.
fn layout_is_pending(cx: &mut App, client_ws: WorkspaceId) -> bool {
    cx.default_global::<TreeSync>()
        .windows
        .get(&client_ws)
        .is_some_and(WindowSync::is_pending)
}


/// Opens the tabs a launch parked here, now that the layout they waited for is
/// up. Does nothing for the windows — every other one — that parked nothing.
fn open_parked_path(cx: &mut App, client_ws: WorkspaceId) {
    let parked = cx
        .default_global::<TreeSync>()
        .windows
        .get_mut(&client_ws)
        .map(WindowSync::take_parked)
        .unwrap_or_default();
    if parked.is_empty() {
        return;
    }
    let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, client_ws) else {
        return;
    };
    let Some(app) =
        crate::ui::windows::WindowRegistry::app_for(cx, client_ws).and_then(|app| app.upgrade())
    else {
        return;
    };
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| {
            for open in parked {
                match open {
                    ParkedOpen::Folder(cwd) => app.new_tab_at(cwd, window, cx),
                    ParkedOpen::Command { cwd, command } => {
                        app.new_tab_running(cwd, command, window, cx)
                    }
                    ParkedOpen::Ssh(ssh) => app.quick_connect(ssh, window, cx),
                }
            }
        });
    });
}

/// A tab parked until its window's layout lands.
enum ParkedOpen {
    /// What a launch and an Explorer double-click ask for.
    Folder(std::path::PathBuf),
    /// A shell in `cwd` with `command` typed into it, from a script or an
    /// `x-man-page:` link Finder handed over.
    Command {
        cwd: std::path::PathBuf,
        command: String,
    },
    /// An `ssh://` link.
    Ssh(lyrel_core::core::ssh_profile::QuickConnect),
}

#[derive(Clone, Copy, PartialEq)]
enum Adopt {
    IfEmpty,
    Replace,
}

/// The tree ids of the tabs `client_ws`'s window is showing.
///
/// Empty for a workspace no window has — and for a test with no registry at
/// all, which is the same answer: nothing is on screen to speak for it.
fn tabs_on_screen(cx: &mut App, client_ws: WorkspaceId) -> Vec<TabId> {
    if !cx.has_global::<crate::ui::windows::WindowRegistry>() {
        return Vec::new();
    }
    crate::ui::windows::WindowRegistry::app_for(cx, client_ws)
        .and_then(|app| app.upgrade())
        .map(|app| app.read(cx).tabs.iter().map(|t| t.id()).collect())
        .unwrap_or_default()
}

fn hydrate(cx: &mut App, client_ws: WorkspaceId, adopt: Adopt) {
    let showing = tabs_on_screen(cx, client_ws);
    hydrate_with(cx, client_ws, adopt, showing);
}

fn hydrate_with(cx: &mut App, client_ws: WorkspaceId, adopt: Adopt, showing: Vec<TabId>) {
    let host = WorkspaceStore::host_of(cx, client_ws);
    let machine_ws = tree_workspace_id(cx, client_ws);
    clear_queue(cx, client_ws);
    let (epoch, failures) = {
        let state = cx
            .default_global::<TreeSync>()
            .windows
            .entry(client_ws)
            .or_default();
        state.begin_hydration(adopt, showing)
    };
    // How many times in a row this window has already failed, which is what
    // decides whether another failure is news or the same news again.
    let level = hydration_log_level(failures, log::Level::Warn);
    cx.spawn(async move |cx| {
        let deadline = std::time::Instant::now() + HYDRATE_LINK_DEADLINE;
        let client = loop {
            match cx.update(|cx| tree_control_for(cx, host)) {
                TreeLink::Ready(client) => break Some(client),
                TreeLink::Unserved => {
                    log::log!(
                        level,
                        "workspace {client_ws}: its machine's server does not serve the \
                         machine tree; opening empty"
                    );
                    break None;
                }
                TreeLink::Down if std::time::Instant::now() > deadline => {
                    log::log!(
                        level,
                        "workspace {client_ws}: no link to its machine; opening empty"
                    );
                    break None;
                }
                TreeLink::Down => cx.background_executor().timer(HYDRATE_LINK_POLL).await,
            }
        };
        let Some(client) = client else {
            cx.update(|cx| {
                // Only the attempt that still owns the window gets to speak: a
                // superseded one is being retried right now, and announcing an
                // emptiness someone else is already filling would be a lie by
                // the time it is read.
                //
                // A machine that answers late is normal for a remote one, and
                // the switcher already says so there. On this computer nothing
                // else would.
                if owe_rehydration(cx, client_ws, epoch, adopt)
                    && adopt == Adopt::IfEmpty
                    && host.is_local()
                {
                    say_why_the_window_is_empty(cx, client_ws);
                }
            });
            return;
        };
        // Read here rather than before the spawn: `switch_workspace` orders
        // this pull and only then names the workspace, so at the moment this
        // task was created the name had not been typed in yet.
        let chosen = cx.update(|cx| {
            cx.default_global::<TreeSync>()
                .windows
                .get(&client_ws)
                .and_then(|state| state.chosen_name(machine_ws))
        });
        let outcome = cx
            .background_executor()
            .spawn(async move { pull_workspace(&client, machine_ws, chosen) })
            .await;
        cx.update(|cx| finish_hydration(cx, client_ws, epoch, adopt, outcome));
    })
    .detach();
}

/// Tells the window it opened empty because its machine never answered.
///
/// An empty window is also what a window with no tabs looks like, and the retry
/// that would fill it in is as quiet as the failure was — so a server one
/// dialect behind reads as "lyrel lost my tabs" with nothing anywhere to say
/// otherwise. This is that "otherwise", said in the window it happened to.
fn say_why_the_window_is_empty(cx: &mut App, client_ws: WorkspaceId) {
    if !cx.default_global::<TreeSync>().windows.get_mut(&client_ws)
        .is_some_and(WindowSync::explain_empty)
    {
        return;
    }
    let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, client_ws) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.notify_app(t(L10nKey::TreeWindowOpenedEmpty), cx);
    });
}

/// Records that a hydration failed and still owes `client_ws` its layout, and
/// arms the retry that pays it back.
///
/// Nothing else recovers on its own: the window stays empty, and without this
/// the next `sync_window` would push that emptiness to the machine as "close
/// every tab". The debt is settled by the next sync of this window — a
/// reconnect drives one through `on_link_up`, an edit in the window drives one
/// through `save_session`, and [`arm_rehydrate_retry`] drives one when neither
/// happens.
///
/// That last driver is the load-bearing one. A pull can fail with the link
/// perfectly healthy — a `MachineGet` that overran its ten seconds on a slow
/// link, or a create that lost its race with `start_prime` — and then no link
/// ever comes back up to notice, and an empty window has nothing to edit. The
/// window sat empty until the app was restarted, with every tab and every
/// shell still on the machine: "lyrel lost my session" for a request that
/// needed asking twice.
///
/// Returns whether the debt was taken on. A superseded attempt gets `false`:
/// a newer hydration owns the window now, and this one speaks for nothing.
fn owe_rehydration(cx: &mut App, client_ws: WorkspaceId, epoch: u64, adopt: Adopt) -> bool {
    let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
        return false;
    };
    let Some(attempts) = state.owe_rehydration(epoch, adopt) else {
        return false;
    };
    log::log!(
        // Once settled this line says the same thing every thirty seconds until
        // the window closes, which is a fact about the machine and not an event.
        hydration_log_level(attempts, log::Level::Info),
        "workspace {client_ws}: will pull its layout again once its machine answers \
         (attempt {attempts})"
    );
    arm_rehydrate_retry(cx, client_ws, epoch, attempts);
    true
}

/// Whether the debt this retry was armed for is still the window's own.
///
/// A newer epoch means another hydration took the window over while the
/// backoff ran, and this retry speaks for nothing.
fn still_owed(cx: &App, client_ws: WorkspaceId, epoch: u64) -> bool {
    cx.try_global::<TreeSync>()
        .and_then(|t| t.windows.get(&client_ws))
        .is_some_and(|s| s.still_owed(epoch))
}

/// The attempt from which the backoff no longer grows.
///
/// Also the point where a window stops being a fresh failure and becomes a
/// standing one, which is what [`hydration_log_level`] keys off.
const REHYDRATE_SETTLED: u32 = 5;
const REHYDRATE_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// The first retry is soon enough to look instant to someone watching an empty
/// window; the backoff is what keeps a machine that is really unreachable from
/// being asked on a loop for as long as its window stays open.
fn rehydrate_backoff(attempts: u32) -> std::time::Duration {
    std::time::Duration::from_secs(2u64.saturating_pow(attempts.min(REHYDRATE_SETTLED)))
        .min(REHYDRATE_BACKOFF_CAP)
}

/// Steps `fresh` down to `debug` once this window's failures have stopped being
/// events and become a standing condition.
///
/// The first few are news: something that was working stopped. Once the backoff
/// has settled at its cap the window is in a steady state — a machine that is
/// simply not there — and the retry will go on failing every thirty seconds for
/// as long as the window stays open. Repeating that at full volume buries
/// whatever else is in the log. The retry stays exactly as persistent either
/// way; only the volume drops.
fn hydration_log_level(attempts: u32, fresh: log::Level) -> log::Level {
    if attempts >= REHYDRATE_SETTLED {
        log::Level::Debug
    } else {
        fresh
    }
}

/// Asks `client_ws` to sync once the backoff is up, if it still owes a pull.
///
/// Deliberately routed through `sync_window` rather than straight into
/// `hydrate`: that is where the rules about *whether* a window may still adopt
/// the machine's layout live — a preempted workspace stays out of it, and a
/// `Replace` is dropped once the user has filled the window in themselves.
fn arm_rehydrate_retry(cx: &mut App, client_ws: WorkspaceId, epoch: u64, attempts: u32) {
    let delay = rehydrate_backoff(attempts);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        let _ = cx.update(|cx| {
            if !still_owed(cx, client_ws, epoch) {
                return;
            }
            // Closing a window drops its `WindowSync` through
            // `forget`, debt and all, so `still_owed` above normally answers
            // first; this covers the window that is on its way out and has
            // already dropped its app.
            let Some(app) = crate::ui::windows::WindowRegistry::app_for(cx, client_ws)
                .and_then(|app| app.upgrade())
            else {
                return;
            };
            app.update(cx, |app, cx| coordinate_window(app, cx));
        });
    })
    .detach();
}

fn pull_workspace(
    client: &ControlClient,
    machine_ws: WorkspaceId,
    chosen: Option<String>,
) -> io::Result<(Machine, WsMirror, Session, Arrival)> {
    // The workspace was on the machine before this pull touched it: adopted,
    // whatever name anyone has parked for it.
    let mut machine = match layout_of(machine_get(client)?, machine_ws) {
        Ok((machine, mirror, session)) => {
            return Ok((machine, mirror, session, Arrival::Adopted));
        }
        Err(machine) => machine,
    };
    // The whole tree is already in hand, so the taken names can be read
    // straight off it rather than passed down from the main thread.
    let taken: Vec<&str> = machine
        .user_workspaces()
        .filter_map(|w| w.name.as_deref())
        .collect();
    // A name the user typed beats a rolled one. This create and `start_prime`'s
    // race each other (see the `Err` arm below), so both have to offer it —
    // whichever wins, the workspace ends up called what was asked for.
    let name = chosen.unwrap_or_else(|| lyrel_core::core::codename::unique(|n| taken.contains(&n)));
    match client.call(ControlRequest::WorkspaceCreate {
        name: Some(name),
        workspace: Some(machine_ws),
    }) {
        // The tree read a moment ago predates the workspace this call just
        // made, and no delta will fill it in: a client is left out of the
        // deltas its own ops raise. Put the created workspace into the tree
        // about to be installed, or the mirror holds it unnamed and the name
        // the machine gave it arrives later looking like a rename (#604).
        Ok(ReplyOk::WorkspaceTree(created)) => {
            machine.workspaces.retain(|w| w.id != created.id);
            machine.workspaces.push(*created);
            Ok((
                machine,
                WsMirror::default(),
                Session::default(),
                Arrival::Created,
            ))
        }
        Ok(_) => Ok((
            machine,
            WsMirror::default(),
            Session::default(),
            Arrival::Created,
        )),
        // Losing this create is not a failed hydration. Opening a remote
        // workspace runs two pulls at once — this one and `start_prime`'s —
        // and both create when the tree they read did not hold it yet, so the
        // loser is told it already exists. The workspace the create was for is
        // on the machine either way, and it may already hold tabs: read the
        // tree again and hydrate from what is really there. Treating this as a
        // failure left the window empty over a workspace that was fine.
        //
        // Any refusal is worth the second look, not just "already exists": what
        // matters is whether the workspace is there now, and the tree answers
        // that better than the error text does. If it still is not there, the
        // create's own refusal is the honest error to report — the reread
        // happened on its behalf and has nothing of its own to say.
        Err(refused) => {
            log::debug!(
                "workspace {machine_ws} could not be created ({refused}); reading the tree \
                 again in case something else created it first"
            );
            // Still `Created`, and deliberately: a create did run for this
            // workspace a moment ago, it just was not this one. That is the
            // race #618 came from — the winner rolled a codename before the
            // user had finished typing — and the typed name is owed against it.
            match machine_get(client) {
                Ok(machine) => layout_of(machine, machine_ws)
                    .map(|(machine, mirror, session)| (machine, mirror, session, Arrival::Created))
                    .map_err(|_| refused),
                Err(_) => Err(refused),
            }
        }
    }
}

fn machine_get(client: &ControlClient) -> io::Result<Machine> {
    match client.call(ControlRequest::MachineGet)? {
        ReplyOk::MachineTree(m) => Ok(*m),
        other => Err(io::Error::other(format!("MachineGet answered {other:?}"))),
    }
}

/// This workspace's layout as `machine` has it, or the tree handed back
/// untouched when the machine does not hold the workspace at all.
fn layout_of(
    machine: Machine,
    machine_ws: WorkspaceId,
) -> Result<(Machine, WsMirror, Session), Machine> {
    let workspace = machine.user_workspaces().find(|w| w.id == machine_ws);
    let Some(ws) = workspace else {
        return Err(machine);
    };
    let mirror = WsMirror {
        tabs: ws.tabs.clone(),
        active: ws.active_tab,
        groups: ws.groups.clone(),
    };
    let session = session_from_tree(ws, &machine.panes);
    Ok((machine, mirror, session))
}

fn finish_hydration(
    cx: &mut App,
    client_ws: WorkspaceId,
    epoch: u64,
    adopt: Adopt,
    outcome: io::Result<(Machine, WsMirror, Session, Arrival)>,
) {
    if settle_hydration(cx, client_ws, epoch, adopt, outcome) {
        open_parked_path(cx, client_ws);
    }
}

/// The body of [`finish_hydration`]. Returns whether this attempt settled the
/// window's layout — false for one that was superseded or has to be retried,
/// which are the two cases where a parked folder waits for the attempt that
/// does settle it rather than opening over a layout still on its way.
fn settle_hydration(
    cx: &mut App,
    client_ws: WorkspaceId,
    epoch: u64,
    adopt: Adopt,
    outcome: io::Result<(Machine, WsMirror, Session, Arrival)>,
) -> bool {
    let current = cx
        .default_global::<TreeSync>()
        .windows
        .get(&client_ws)
        .map(WindowSync::epoch);
    if current != Some(epoch) {
        log::debug!("workspace {client_ws}: dropping a superseded hydration");
        return false;
    }
    let (machine, mirror, session, arrival) = match outcome {
        Ok(pulled) => pulled,
        Err(e) => {
            let failures = cx
                .default_global::<TreeSync>()
                .windows
                .get(&client_ws)
                .map_or(0, WindowSync::failures);
            log::log!(
                hydration_log_level(failures, log::Level::Warn),
                "could not hydrate workspace {client_ws} from its machine: {e}"
            );
            let _ = owe_rehydration(cx, client_ws, epoch, adopt);
            return false;
        }
    };
    let host = WorkspaceStore::host_of(cx, client_ws);
    let machine_ws = tree_workspace_id(cx, client_ws);
    // A workspace on this computer that another client is driving keeps its
    // panes to that client: attaching to them here, even for the moment before
    // the takeover is noticed, would resize them under it. A window restored or
    // reconnected onto one opens taken over instead, with Take Back.
    if host.is_local() {
        let holder = machine
            .user_workspaces()
            .find(|w| w.id == machine_ws)
            .and_then(|w| w.attachment.as_ref())
            .map(|a| a.hostname.clone());
        if !crate::ui::local_link::LocalLink::hydration_may_proceed(
            cx,
            client_ws,
            holder.as_deref(),
        ) {
            crate::ui::machine_mirror::MachineMirrors::install(cx, host, machine);
            // Unprimed and not priming: the window speaks for nothing until a
            // Take Back rebuilds it from the tree.
            on_preempted(cx, client_ws);
            return false;
        }
    }
    // What the tree that is about to be installed calls this workspace, which
    // for a pull that had to create it is the name that create proposed.
    let answered = machine
        .user_workspaces()
        .find(|w| w.id == machine_ws)
        .and_then(|w| w.name.clone());
    crate::ui::machine_mirror::MachineMirrors::install(cx, host, machine);
    let name = settle_chosen_name(cx, client_ws, answered, arrival);
    crate::ui::machine_mirror::MachineMirrors::note_workspace_name(cx, host, machine_ws, name);
    let showing = tabs_on_screen(cx, client_ws);
    let landing = {
        let Some(state) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws) else {
            return false;
        };
        state.hydration_landed(mirror, adopt, &showing)
    };
    adopt_groups(cx, client_ws);
    let Some(app) =
        crate::ui::windows::WindowRegistry::app_for(cx, client_ws).and_then(|app| app.upgrade())
    else {
        return false;
    };
    if landing.keep_window {
        // A full window over an empty tree has to write itself back, whether
        // or not an edit was waiting: the machine is missing tabs this window
        // is showing, and nothing else would ever put them there.
        //
        // Deliberately not limited to this machine. An empty tree means one of
        // two things and the answer is the same either way: locally the
        // workspace was removed under the window (`ws rm`, or another client),
        // and remotely the far end lost its records — a re-imaged box, a store
        // that was wiped. Writing the window back is what a reattach is for.
        // The panes it names may well be dead; the window already draws them
        // that way, and a tab the user can close beats a tab that silently
        // stops existing.
        if landing.publish_window {
            app.update(cx, |app, cx| coordinate_window(app, cx));
        }
        return true;
    }
    if session.tabs.is_empty() && adopt == Adopt::IfEmpty {
        if landing.dirty
            && let Some(app) =
                crate::ui::windows::WindowRegistry::app_for(cx, client_ws).and_then(|a| a.upgrade())
        {
            app.update(cx, |app, cx| coordinate_window(app, cx));
        }
        return true;
    }
    let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, client_ws) else {
        return false;
    };
    let wanted = session.tabs.len();
    // `tree_id` is not serialized, so only a session built from the tree
    // carries one on every tab (which is what reaches here today). The guard
    // below counts the tabs asked for, not the ids found: a session with tabs
    // and no ids must not read as "nothing was wanted".
    let wanted_ids: Vec<TabId> = session.tabs.iter().filter_map(|t| t.tree_id).collect();
    debug_assert_eq!(
        wanted_ids.len(),
        wanted,
        "a session rebuilt from the tree names every tab it holds"
    );
    log::info!("rebuilding {wanted} tab(s) of workspace {client_ws} from its machine's tree");
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| {
            app.adopt_workspace(client_ws, session, window, cx)
        });
    });
    let showing = tabs_on_screen(cx, client_ws);
    settle_rebuild(cx, client_ws, wanted, &wanted_ids, &showing);
    true
}

/// What a rebuild leaves the window entitled to say, from how many tabs the
/// tree asked it to put up (`wanted`), which ids those were (`wanted_ids`),
/// and the tabs it is showing now (`showing`).
///
/// Informed *after* the rebuild, and only if the rebuild produced something.
///
/// The licence means "this window knows what belongs in this workspace", and
/// `switch_workspace` / `detach_workspace` read it as permission to delete a
/// workspace that has no tabs — from the machine tree and from the store
/// both. Granting it before the rebuild handed that permission to a window
/// whose rebuild had not happened yet, and a rebuild can produce nothing:
/// `tabs_from_session` drops any tab whose panes all fail to start, which is
/// what every tab does when the pane socket is unreachable. The window then
/// sat there, empty and authoritative, and the next switch deleted a
/// workspace with ten live tabs in it.
///
/// Emptiness that came from a failure has to stay indistinguishable from not
/// knowing, because that is what it is.
///
/// A rebuild that produced *some* of its tabs is the same failure, one tab at
/// a time, and the licence it does earn — it put a layout up, and closes and
/// splits in it must reach the machine — cannot be allowed to speak for the
/// tabs it did not: at `SyncScope::Full` every mirror tab missing from the
/// window is a `TabClose`, and the tabs missing were exactly the ones that
/// failed to rebuild, so the sync deleted them off the machine, panes and all
/// (#672). They are held instead (`not_rebuilt`), out of the diff's reach until
/// a later rebuild puts them up or the machine lets them go.
///
/// Holding has a cost the caller should know: `diff` stops before its
/// reorder pass and the active-tab op whenever anything is held, so while
/// this set stands the window's tab order and active tab do not reach the
/// machine — a restart restores the order and focus from before the drag.
/// And nothing retries a held tab: the set is rewritten only by the next
/// `settle_rebuild`, which runs only from a hydration that rebuilds, and a
/// re-prime or an `Adopt::IfEmpty` hydrate on a populated window never gets
/// there. A tab that fails to rebuild stays held, and holds the ordering
/// with it, until the next restart. That state was already reachable — a
/// pane whose remote spawn failed stays connecting for the same span, held
/// the same way — so this widens a standing hole rather than opening one; a
/// retry, or a way to close a held tab from the window, is separate work.
///
/// The none-rebuilt guard reads the *count* asked for, not the ids found:
/// `tree_id` is not serialized, so a session that reached here from disk
/// would name no ids at all, and "no ids" must not read as "no tabs wanted"
/// — that would grant the licence to a window that rebuilt nothing, which is
/// #672 again.
fn settle_rebuild(
    cx: &mut App,
    client_ws: WorkspaceId,
    wanted: usize,
    wanted_ids: &[TabId],
    showing: &[TabId],
) {
    let failed = cx.default_global::<TreeSync>().windows.entry(client_ws).or_default()
        .settle_rebuild(wanted, wanted_ids, showing);
    if failed != 0 && !showing.is_empty() {
        log::warn!(
            "workspace {client_ws}: {} of its {wanted} tab(s) could not be rebuilt; they stay on \
             the machine, held out of this window's sync (tab order and active tab are not \
             synced while a tab is held)",
            failed
        );
    }
    log::info!(
        "workspace {client_ws} rebuild complete; requested={wanted} restored={} failed={}",
        wanted.saturating_sub(failed),
        failed
    );
    if showing.is_empty() && wanted != 0 {
        log::warn!(
            "workspace {client_ws}: none of its {wanted} tab(s) could be rebuilt; leaving the \
             window uninformed so the layout is not mistaken for an empty workspace"
        );
    }
}

/// Someone else removed this workspace from its machine — `lyrel ws rm`, or
/// another client.
///
/// With no window on it, it stops existing here too. Left in the store it
/// would keep its row in the switcher and open onto nothing, which is how a
/// workspace deleted from the CLI used to haunt the panel until a restart.
///
/// With a window on it, the window stays: `ws rm` leaves every pane running,
/// and closing the window would strand them with no way back. Pulling the
/// layout again is what makes that honest — finding the workspace gone is
/// exactly the case `pull_workspace` puts back under the same id, and the
/// window writes its tabs to it on the way out of the hydration.
fn on_workspace_deleted(cx: &mut App, client_ws: WorkspaceId) {
    if crate::ui::windows::WindowRegistry::window_for(cx, client_ws).is_none() {
        log::info!("workspace {client_ws} was deleted on its machine; forgetting it here too");
        forget(cx, client_ws);
        crate::core::session::WorkspaceStore::remove(cx, client_ws);
        crate::ui::windows::refresh_menu(cx);
        cx.refresh_windows();
        return;
    }
    log::info!(
        "workspace {client_ws} was deleted on its machine while a window still had it open; \
         putting it back under the same id"
    );
    hydrate(cx, client_ws, Adopt::IfEmpty);
}

pub(crate) fn on_layout_delta(cx: &mut App, host: HostId, key: &str, delta: LayoutDelta) {
    if host.is_local()
        && let LayoutDelta::WorkspaceCreated { workspace } = &delta
        && workspace.foreign_view.is_some()
    {
        crate::ui::machine_mirror::MachineMirrors::apply_delta(cx, host, key, &delta);
        crate::ui::pane_sidecar::on_sidecar_delta(cx, workspace);
        return;
    }
    // Asked while the mirror still holds the pane as it was.
    let nothing_shown =
        crate::ui::machine_mirror::MachineMirrors::delta_changes_nothing_shown(cx, host, &delta);
    crate::ui::machine_mirror::MachineMirrors::apply_delta(cx, host, key, &delta);
    let client_ws = if host.is_local() {
        key.parse::<WorkspaceId>().ok()
    } else {
        WorkspaceStore::all(cx)
            .views
            .iter()
            .find(|w| {
                w.host
                    .as_ref()
                    .is_some_and(|r| r.host_id() == host && r.workspace.to_string() == key)
            })
            .map(|w| w.id)
    };
    let Some(client_ws) = client_ws else {
        return;
    };

    if crate::ui::remote_workspace::workspace_is_preempted(cx, client_ws) {
        on_preempted(cx, client_ws);
        return;
    }

    if matches!(delta, LayoutDelta::WorkspaceDeleted) {
        on_workspace_deleted(cx, client_ws);
        return;
    }

    let Some(mirror_ok) = cx.default_global::<TreeSync>().windows.get_mut(&client_ws)
        .and_then(|state| state.apply_delta(&delta))
    else {
        return;
    };

    let Some(app) =
        crate::ui::windows::WindowRegistry::app_for(cx, client_ws).and_then(|a| a.upgrade())
    else {
        return;
    };
    let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, client_ws) else {
        return;
    };
    // `apply_layout_delta` ends in a `notify()` of the whole window, and for a
    // pane-facts delta that is all it does. One that changes nothing a tab
    // shows (an agent's spinner turning a quarter) has no use for the repaint.
    let window_ok = nothing_shown
        || handle
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| app.apply_layout_delta(&delta, window, cx))
            })
            .unwrap_or(true);
    if !mirror_ok || !window_ok {
        log::info!(
            "workspace {client_ws}: delta {delta:?} did not apply cleanly; re-pulling the tree"
        );
        resync_window_from_tree(cx, client_ws);
        return;
    }
    app.update(cx, |app, cx| coordinate_window(app, cx));
}


pub(crate) fn resync_window_from_tree(cx: &mut App, client_ws: WorkspaceId) {
    hydrate(cx, client_ws, Adopt::Replace);
}

/// Records the daemon process a link is talking to, answering "did the server
/// behind this link just become a *different* process?".
///
/// `seen` is the instance last recorded for this link, empty when unknown. A
/// first sighting is not a restart (there is nothing on screen to be wrong
/// about yet), and an empty instance means the server predates the field —
/// it neither reports nor overwrites what was seen before. Shared by the
/// remote path (`remote_workspace::server_restarted`, keyed per host) and the
/// local link (single daemon), which needs the same comparison to notice the
/// daemon it lost came back as another process (#553).
pub(crate) fn note_instance(seen: &mut String, instance: &str) -> bool {
    if instance.is_empty() {
        return false;
    }
    let before = std::mem::replace(seen, instance.to_string());
    if !before.is_empty() && before != instance {
        log::info!(
            "the lyrel-server on this machine is a new process ({before} → {instance}); \
             its panes are gone"
        );
        return true;
    }
    false
}

/// Drops the link to the local daemon and rebuilds every local window from the
/// machine tree, for a caller that killed that daemon itself (#553).
///
/// The invalidate half comes first on purpose: the link still holds the client
/// that pointed at the server that is now gone, and a pull sent down it dies on
/// a dead socket before the reader notices. With it dropped, `hydrate` waits for
/// the reconnect `LocalLink::tick` is already driving and pulls the layout the
/// daemon actually has.
///
/// Only for the caller that has no live link left. One that just handshaked a
/// *new* daemon calls [`resync_local_windows_from_tree`] with that link in hand:
/// dropping it there would throw away a working link and make every window wait
/// out another connect for no reason.
pub(crate) fn resync_after_local_daemon_change(cx: &mut App) {
    crate::ui::local_link::LocalLink::invalidate(cx);
    resync_local_windows_from_tree(cx);
}

/// Rebuilds every local window from the machine tree, because the daemon behind
/// the local link became a different process — it died and the reconnect found a
/// new one, whose registry knows nothing about the panes on screen (#553).
///
/// Remote windows are left alone: their own link says when their machine's
/// server changed, and this one speaks for this computer only.
///
/// When no daemon answers the pull, the windows owe a rehydration instead
/// (`owe_rehydration`) — which is also the guard that keeps a window emptied by
/// a failed pull from being pushed back up as "close every tab".
pub(crate) fn resync_local_windows_from_tree(cx: &mut App) {
    for (workspace, _) in crate::ui::windows::WindowRegistry::open_windows(cx) {
        if WorkspaceStore::host_of(cx, workspace) != HostId::LOCAL {
            continue;
        }
        // Taken over: its panes belong to another client, and only a Take
        // Back rebuilds it.
        if crate::ui::remote_workspace::workspace_is_preempted(cx, workspace) {
            continue;
        }
        resync_window_from_tree(cx, workspace);
    }
}

impl LyrelApp {
    pub(crate) fn apply_layout_delta(
        &mut self,
        delta: &LayoutDelta,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> bool {
        let index_of = |tabs: &[crate::ui::tab::Tab], id: TabId| {
            tabs.iter().position(|t| t.id() == id)
        };
        let applied = match delta {
            LayoutDelta::WorkspaceCreated { .. }
            | LayoutDelta::WorkspaceTouched { .. }
            | LayoutDelta::WorkspaceRenamed { .. }
            | LayoutDelta::PaneFacts { .. } => true,
            // Unreachable: `on_layout_delta` hands a deletion to
            // `on_workspace_deleted` and returns before any window is asked. A
            // deletion is about whether this workspace still exists here at
            // all, which is not a question one window's tab list can answer.
            LayoutDelta::WorkspaceDeleted => true,
            LayoutDelta::ActiveTabChanged { tab } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    self.activate_from_delta(index, window, cx);
                }
                true
            }
            // A tab arriving asleep, or one that someone else put to sleep or
            // woke, is not a shape to graft onto this window: building it here
            // would spawn what is meant to stay stopped, or leave running what
            // was just stopped. Saying the delta did not apply re-pulls the
            // tree, and the restore that follows is the one path that already
            // knows what a sleeping tab is.
            LayoutDelta::TabCreated { tab, .. } if tab.hibernated => false,
            LayoutDelta::TabCreated { at, tab } => {
                self.insert_tab_from_tree((*at).min(self.tabs.len()), tab, window, cx)
            }
            LayoutDelta::TabClosed { tab, reason } => {
                if *reason == lyrel_core::core::machine::TabCloseReason::Explicit {
                    crate::ui::pane_sidecar::intend_close(cx, self.workspace, *tab);
                }
                if let Some(index) = index_of(&self.tabs, *tab) {
                    let removed_focus = self.tabs[index].pane.focused_leaf(window, cx).is_some();
                    if *reason == lyrel_core::core::machine::TabCloseReason::Emptied
                        && crate::ui::pane_sidecar::retain_client_local(&mut self.tabs[index], cx)
                    {
                        self.tabs[index].clear_zoom();
                        self.maximized = None;
                        if removed_focus {
                            self.focus_active(window, cx);
                        }
                        crate::ui::pane_sidecar::sync_window(self, cx);
                        self.refocus_parked_pane(window, cx);
                        cx.notify();
                        return true;
                    }
                    self.tabs.remove(index);
                    self.maximized = None;
                    self.wake_active_if_asleep(window, cx);
                    self.focus_active(window, cx);
                }
                true
            }
            LayoutDelta::TabRenamed { tab, name } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    self.tabs[index].adopt_name(name.clone());
                }
                true
            }
            LayoutDelta::TabRegrouped {
                tab,
                group,
                last_auto,
            } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    let gui = &self.tabs[index];
                    gui.group.set(*group);
                    // Another window's hint only fills a gap. Where this
                    // window has an answer of its own it keeps it: the two
                    // probe the same cwd and agree, and letting each
                    // overwrite the other would bounce a disagreement between
                    // them for as long as it lasted.
                    if gui.auto_group.borrow().is_none() {
                        *gui.auto_group.borrow_mut() = last_auto.clone();
                    }
                }
                true
            }
            LayoutDelta::GroupsChanged { groups } => {
                self.adopt_sidebar_groups(groups.clone(), cx);
                true
            }
            LayoutDelta::TabMoved { tab, to } => {
                self.tabs.move_to(*tab, *to);
                true
            }
            LayoutDelta::TabRestructured { tab, .. } => match index_of(&self.tabs, tab.id) {
                Some(index) if tab.hibernated || self.tabs[index].is_asleep() => false,
                Some(index) => self.rebuild_tab_from_tree(index, tab, window, cx),
                None => false,
            },
            LayoutDelta::RatioChanged { tab, path, ratio } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    set_gui_ratio(&mut self.tabs[index].pane, path, *ratio)
                } else {
                    true
                }
            }
        };
        if applied {
            self.refocus_parked_pane(window, cx);
        }
        cx.notify();
        applied
    }

    fn activate_from_delta(
        &mut self,
        index: usize,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        if self.tabs.active_index() == index {
            return;
        }
        self.maximized = None;
        self.tabs.select(index);
        self.wake_active_if_asleep(window, cx);
        self.focus_active(window, cx);
    }

    fn insert_tab_from_tree(
        &mut self,
        at: usize,
        tab: &TreeTab,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> bool {
        if self.tabs.iter().any(|t| t.id() == tab.id) {
            return true;
        }
        let mut existing = HashMap::new();
        let Some(pane) = self.build_pane_from_tree(&tab.root, &mut existing, window, cx) else {
            return false;
        };
        let gui = crate::ui::tab::Tab::from_tree(tab, pane);
        self.tabs.insert(at, gui);
        true
    }

    fn rebuild_tab_from_tree(
        &mut self,
        index: usize,
        tab: &TreeTab,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> bool {
        let remote = WorkspaceStore::all(cx)
            .get(self.workspace)
            .is_some_and(|w| w.is_remote());
        if remote {
            crate::ui::pane_sidecar::note_primary_tab(&mut self.tabs[index], tab, cx);
        }
        let presentation = tab.presentation.clone();
        if logical_node(&self.tabs[index].pane, remote, cx).as_ref() == Some(&tab.root) {
            let gui = &mut self.tabs[index];
            gui.presentation = presentation;
            gui.adopt_name(tab.name.clone());
            gui.group.set(tab.group);
            if gui.auto_group.borrow().is_none() {
                *gui.auto_group.borrow_mut() = tab.last_auto.clone();
            }
            return true;
        }
        let mut existing: HashMap<u64, PaneSlot> = HashMap::new();
        let mut ssh_slots: Vec<PaneSlot> = Vec::new();
        for slot in self.tabs[index].pane.leaves() {
            let id = match &slot {
                PaneSlot::Ready(view) if remote && view.read(cx).ssh_spec().is_some() => {
                    ssh_slots.push(slot);
                    continue;
                }
                PaneSlot::Ready(view) => Some(view.read(cx).pane_id),
                PaneSlot::Connecting(pending) => pending.read(cx).restore_pane(),
            };
            if let Some(id) = id {
                existing.insert(id, slot);
            }
        }
        let Some(pane) = self.build_pane_from_tree(&tab.root, &mut existing, window, cx) else {
            return false;
        };
        let pane = ssh_slots.into_iter().fold(pane, |tree, slot| {
            Pane::split_node(gpui_kit::Axis::Horizontal, 0.5, tree, Pane::Leaf(slot))
        });
        let gui = &mut self.tabs[index];
        gui.pane = pane;
        gui.presentation = presentation;
        gui.adopt_name(tab.name.clone());
        gui.group.set(tab.group);
        if gui.auto_group.borrow().is_none() {
            *gui.auto_group.borrow_mut() = tab.last_auto.clone();
        }
        self.maximized = None;
        true
    }

    fn build_pane_from_tree(
        &self,
        node: &PaneNode,
        existing: &mut HashMap<u64, PaneSlot>,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Option<Pane> {
        match node {
            PaneNode::Leaf { pane } => {
                if let Some(slot) = existing.remove(pane) {
                    return Some(Pane::Leaf(slot));
                }
                match crate::ui::app::new_terminal(
                    self.window_workspace(cx),
                    Some(self.workspace),
                    self.font_size,
                    None,
                    Some(*pane),
                    None,
                    window,
                    cx,
                ) {
                    Ok(slot) => Some(Pane::Leaf(slot)),
                    Err(e) => {
                        log::warn!("could not attach pane {pane} from a delta: {e}");
                        None
                    }
                }
            }
            PaneNode::Split { axis, ratio, a, b } => {
                let left = self.build_pane_from_tree(a, existing, window, cx);
                let right = self.build_pane_from_tree(b, existing, window, cx);
                match (left, right) {
                    (Some(a), Some(b)) => Some(Pane::split_node(
                        match axis {
                            TreeAxis::Horizontal => gpui_kit::Axis::Horizontal,
                            TreeAxis::Vertical => gpui_kit::Axis::Vertical,
                        },
                        *ratio,
                        a,
                        b,
                    )),
                    (one, other) => one.or(other),
                }
            }
        }
    }
}

fn set_gui_ratio(pane: &mut Pane, path: &[Side], ratio: f32) -> bool {
    match path.split_first() {
        None => match pane {
            Pane::Split { ratio: cell, .. } => {
                cell.set(ratio.clamp(0.05, 0.95));
                true
            }
            _ => false,
        },
        Some((side, rest)) => match pane {
            Pane::Split { a, b, .. } => match side {
                Side::A => set_gui_ratio(a, rest, ratio),
                Side::B => set_gui_ratio(b, rest, ratio),
            },
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::core::group_key::{AutoKey, GroupId};
    use lyrel_core::core::machine::{AgentFacts, TabPresentation};


    /// A reopened tab comes back as the restore reads one: same shape, each
    /// leaf on its old pane id — the key its screen is restored by — with the
    /// cwd, shell, SSH target and agent the machine kept, under its old id.
    #[test]
    fn a_reopened_tab_reads_back_its_layout_and_what_each_pane_was() {
        use lyrel_core::core::machine::{PaneRecord, ReopenedTab};
        let mut tab = TreeTab {
            name: Some("api".into()),
            root: PaneNode::Split {
                axis: TreeAxis::Vertical,
                ratio: 0.3,
                a: Box::new(PaneNode::Leaf { pane: 7 }),
                b: Box::new(PaneNode::Leaf { pane: 8 }),
            },
            ..TreeTab::leaf(0)
        };
        assert!(tab.set_pip(Some(8), Some(7)));
        let mut agent_pane = PaneRecord::new(8);
        agent_pane.cwd = Some("/repo".into());
        agent_pane.agent = Some(AgentFacts {
            agent: crate::core::cli_agent::CLIAgent::OhMyPi,
            session_id: Some("abc".into()),
            launch_argv: None,
            session_scope: None,
            status: None,
        });
        let mut shell_pane = PaneRecord::new(7);
        shell_pane.cwd = Some("/srv".into());
        let reopened = ReopenedTab {
            tab: tab.clone(),
            closed_at: 1,
            panes: vec![shell_pane, agent_pane],
        };

        let st = session_tab_from_reopened(&reopened);
        assert_eq!(st.presentation, tab.presentation);

        assert_eq!(st.tree_id, Some(tab.id));
        assert_eq!(st.name.as_deref(), Some("api"));
        assert!(!st.hibernated);
        let SessionPane::Split { ratio, a, b, .. } = st.pane else {
            panic!("the split comes back a split");
        };
        assert_eq!(ratio, 0.3);
        let SessionPane::Leaf { pane_id, cwd, .. } = *a else {
            panic!("leaf");
        };
        assert_eq!(pane_id, Some(7));
        assert_eq!(cwd, Some(std::path::PathBuf::from("/srv")));
        let SessionPane::Leaf {
            pane_id,
            agent,
            agent_session_id,
            ..
        } = *b
        else {
            panic!("leaf");
        };
        assert_eq!(pane_id, Some(8));
        assert_eq!(agent, Some(crate::core::cli_agent::CLIAgent::OhMyPi));
        assert_eq!(agent_session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn note_instance_reports_only_a_real_change() {
        let mut seen = String::new();

        assert!(
            !note_instance(&mut seen, "abc"),
            "a first sighting is not a restart"
        );
        assert!(
            !note_instance(&mut seen, "abc"),
            "the same process is not a restart"
        );
        assert!(note_instance(&mut seen, "def"), "a new process is");
        assert!(!note_instance(&mut seen, "def"));
    }

    #[test]
    fn note_instance_ignores_a_server_that_predates_the_field() {
        let mut seen = String::from("abc");

        assert!(
            !note_instance(&mut seen, ""),
            "an unknown instance is never a restart"
        );
        assert_eq!(seen, "abc", "and it must not overwrite what was seen");

        let mut fresh = String::new();
        assert!(!note_instance(&mut fresh, ""));
        assert!(
            !note_instance(&mut fresh, "abc"),
            "the first real instance after an unknown one is a first sighting"
        );
    }


    #[cfg(unix)]
    #[test]
    fn a_peer_without_the_machine_tree_bit_classifies_as_unserved() {
        use lyrel_core::daemon::control::ControlHello;
        use lyrel_core::host::local::LocalHost;
        use lyrel_core::host::server::{Services, serve_with};

        let connect = |services: Services| {
            let (server, client) = std::os::unix::net::UnixStream::pair().unwrap();
            std::thread::spawn(move || {
                let _ = serve_with(server, LocalHost::new(), services);
            });
            let hello = ControlHello::host_rpc("test-token", "test-host");
            Arc::new(
                lyrel_core::daemon::control::ControlClient::over_unix(
                    client,
                    &hello,
                    Box::new(|_| {}),
                )
                .unwrap(),
            )
        };

        let treeless = connect(Services::none());
        assert!(matches!(
            classify_tree_link(Some(treeless)),
            TreeLink::Unserved
        ));

        let dir = std::env::temp_dir().join(format!("lyrel-treelink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = lyrel_core::core::machine::MachineStore::open(
            dir.join(lyrel_core::core::machine::MACHINE_FILE),
        );
        let serving = connect(Services::with_machine(store));
        assert!(matches!(
            classify_tree_link(Some(serving)),
            TreeLink::Ready(_)
        ));

        assert!(matches!(classify_tree_link(None), TreeLink::Down));
        let _ = std::fs::remove_dir_all(&dir);
    }



    #[test]
    fn a_ratio_delta_is_clamped_to_the_servers_band_not_a_narrower_one() {
        let mut pane = Pane::split_node(gpui_kit::Axis::Horizontal, 0.5, Pane::Empty, Pane::Empty);
        assert!(set_gui_ratio(&mut pane, &[], 0.07));
        match &pane {
            Pane::Split { ratio, .. } => assert_eq!(ratio.get(), 0.07),
            _ => unreachable!("built as a split"),
        }
        assert!(set_gui_ratio(&mut pane, &[], 0.01));
        match &pane {
            Pane::Split { ratio, .. } => assert_eq!(ratio.get(), 0.05),
            _ => unreachable!("built as a split"),
        }
    }


    #[test]
    fn a_lowered_leaf_carries_its_pane_id_and_its_agent_whatever_live_says() {
        use lyrel_core::core::cli_agent::CLIAgent;
        let tab_id = TabId::new();
        let ws = lyrel_core::core::machine::Workspace {
            tabs: vec![TreeTab {
                id: tab_id,
                name: Some("build".into()),
                group: Some(GroupId::new()),
                last_auto: Some(AutoKey::Repo("/work".into())),
                presentation: TabPresentation::default(),
                root: PaneNode::Split {
                    axis: TreeAxis::Vertical,
                    ratio: 0.3,
                    a: Box::new(PaneNode::Leaf { pane: 1 }),
                    b: Box::new(PaneNode::Leaf { pane: 2 }),
                },
                hibernated: false,
            }],
            active_tab: Some(tab_id),
            ..Default::default()
        };
        let panes = vec![
            PaneRecord {
                id: 1,
                cwd: Some("/work".into()),
                live: true,
                ..PaneRecord::new(1)
            },
            PaneRecord {
                id: 2,
                cwd: Some("/work/api".into()),
                live: false,
                agent: Some(AgentFacts {
                    agent: CLIAgent::OhMyPi,
                    session_id: Some("sid".into()),
                    launch_argv: Some(vec!["omp".into()]),
                    session_scope: None,
                    status: None,
                }),
                ..PaneRecord::new(2)
            },
        ];

        let session = session_from_tree(&ws, &panes);
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.active, 0);
        let tab = &session.tabs[0];
        assert_eq!(
            tab.tree_id,
            Some(tab_id),
            "the daemon tab's identity rides along"
        );
        assert_eq!(tab.name.as_deref(), Some("build"));
        assert_eq!(
            tab.last_auto,
            Some(AutoKey::Repo("/work".into())),
            "the auto-group hint rides along, so the tab is drawn in its group at once"
        );
        let SessionPane::Split { ratio, a, b, .. } = &tab.pane else {
            panic!("the split survives the lowering");
        };
        assert!((ratio - 0.3).abs() < 1e-6);
        match &**a {
            SessionPane::Leaf { pane_id, cwd, .. } => {
                assert_eq!(*pane_id, Some(1), "a live pane re-attaches by its id");
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/work")));
            }
            _ => panic!("leaf"),
        }
        match &**b {
            SessionPane::Leaf {
                pane_id,
                cwd,
                agent,
                agent_session_id,
                ..
            } => {
                assert_eq!(
                    *pane_id,
                    Some(2),
                    "a pane the tree calls dead still goes down by its id: the flag is a \
                     cached observation from another process — reloaded as false on every \
                     server start — and attaching is what settles it. Believing the flag \
                     here spawned fresh shells over running sessions."
                );
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/work/api")));
                assert_eq!(*agent, Some(CLIAgent::OhMyPi));
                assert_eq!(agent_session_id.as_deref(), Some("sid"));
            }
            _ => panic!("leaf"),
        }
    }

    /// The regression behind "switching workspaces threw away every session".
    ///
    /// Two servers had started against one config dir — one holding the control
    /// socket with an empty pane registry, the other holding the panes — so
    /// `MachineGet` answered with every `live` still `false`, the value
    /// `load_machine` stamps on a cold read. Erasing the id on that made the
    /// restore spawn a fresh shell over each running one, and nineteen live
    /// agent sessions went out with it.
    ///
    /// The id has to survive a `live: false`, because nothing here is entitled
    /// to declare a pane dead. Attaching is.
    #[test]
    fn a_pane_the_tree_calls_dead_still_goes_down_by_its_id() {
        let tab_id = TabId::new();
        let ws = lyrel_core::core::machine::Workspace {
            tabs: vec![TreeTab {
                id: tab_id,
                name: None,
                group: None,
                last_auto: None,
                presentation: TabPresentation::default(),
                root: PaneNode::Leaf { pane: 7 },
                hibernated: false,
            }],
            active_tab: Some(tab_id),
            ..Default::default()
        };
        let panes = vec![PaneRecord {
            id: 7,
            cwd: Some("/work".into()),
            live: false,
            ..PaneRecord::new(7)
        }];

        match &session_from_tree(&ws, &panes).tabs[0].pane {
            SessionPane::Leaf { pane_id, .. } => assert_eq!(
                *pane_id,
                Some(7),
                "the attach decides whether pane 7 is still there; this must not pre-empt it"
            ),
            _ => panic!("leaf"),
        }
    }

    #[test]
    fn a_dangling_active_tab_in_the_pulled_tree_falls_back_to_the_first() {
        let ws = lyrel_core::core::machine::Workspace {
            tabs: vec![TreeTab {
                id: TabId::new(),
                name: None,
                group: None,
                last_auto: None,
                presentation: TabPresentation::default(),
                root: PaneNode::Leaf { pane: 1 },
                hibernated: false,
            }],
            active_tab: Some(TabId::new()),
            ..Default::default()
        };
        assert_eq!(session_from_tree(&ws, &[]).active, 0);
    }

    /// A restore reads the mark off the tree: the sleeping tab keeps its pane
    /// ids — they are what the wake restores from — and comes with a name read
    /// off the tree's records, since nothing in it will be running to say one.
    #[test]
    fn a_restore_brings_a_sleeping_tab_back_asleep_and_named() {
        let (awake, sleeping) = (TabId::new(), TabId::new());
        let ws = lyrel_core::core::machine::Workspace {
            tabs: vec![
                TreeTab {
                    id: awake,
                    name: None,
                    group: None,
                    last_auto: None,
                    presentation: TabPresentation::default(),
                    root: PaneNode::Leaf { pane: 1 },
                    hibernated: false,
                },
                TreeTab {
                    id: sleeping,
                    name: None,
                    group: None,
                    last_auto: None,
                    presentation: TabPresentation::default(),
                    root: PaneNode::Leaf { pane: 2 },
                    hibernated: true,
                },
            ],
            active_tab: Some(awake),
            ..Default::default()
        };
        let panes = vec![
            PaneRecord {
                cwd: Some("/work".into()),
                live: true,
                ..PaneRecord::new(1)
            },
            PaneRecord {
                cwd: Some("/work/api".into()),
                osc_title: Some("fixing the switcher".into()),
                live: false,
                ..PaneRecord::new(2)
            },
        ];

        let session = session_from_tree(&ws, &panes);
        assert!(!session.tabs[0].hibernated);
        assert!(session.tabs[0].asleep_view.is_none());
        let tab = &session.tabs[1];
        assert!(tab.hibernated);
        match &tab.pane {
            SessionPane::Leaf { pane_id, cwd, .. } => {
                assert_eq!(*pane_id, Some(2));
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/work/api")));
            }
            _ => panic!("leaf"),
        }
        let view = tab.asleep_view.as_ref().expect("a sleeping tab is named");
        assert_eq!(view.osc_title.as_deref(), Some("fixing the switcher"));
        assert!(!view.live);
    }
}
