use super::super::test_support::*;
use super::*;
use crate::core::group_key::GroupId;
use crate::core::machine::{Axis, CLOSED_TAB_TTL, MAX_CLOSED_TABS, PaneSeed, TabCloseReason, TabId};

#[test]
fn a_client_minted_tab_id_is_kept_and_a_duplicate_is_refused_anywhere() {
    let (store, _dir, ws, _tab) = store_with_tab();
    let id = TabId::new();
    let tab = store
        .tab_create(ws, None, seed(2, "/b"), Some(id), None)
        .unwrap();
    assert_eq!(tab.id, id);

    let other = store.workspace_create(None, None, None).unwrap();
    let refused = store
        .tab_create(other.id, None, seed(3, "/c"), Some(id), None)
        .expect_err("a taken tab id must refuse");
    assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    assert!(
        store.pane(3).is_none(),
        "the refused create adopted no pane either"
    );
}

#[test]
fn a_created_tab_lands_at_its_position_and_becomes_active() {
    let (store, _dir, ws, first) = store_with_tab();
    let second = store
        .tab_create(ws, None, seed(2, "/b"), None, None)
        .unwrap();
    let between = store
        .tab_create(ws, Some(1), seed(3, "/c"), None, None)
        .unwrap();

    let workspace = store.workspace(ws).unwrap();
    let order: Vec<TabId> = workspace.tabs.iter().map(|t| t.id).collect();
    assert_eq!(order, vec![first.id, between.id, second.id]);
    assert_eq!(workspace.active_tab, Some(between.id));

    let clamped = store
        .tab_create(ws, Some(99), seed(4, "/d"), None, None)
        .unwrap();
    assert_eq!(
        store.workspace(ws).unwrap().tabs.last().unwrap().id,
        clamped.id
    );
}

#[test]
fn closing_a_tab_forgets_its_panes_and_heals_the_active_tab() {
    let (store, _dir, ws, first) = store_with_tab();
    let second = store
        .tab_create(ws, None, seed(2, "/b"), None, None)
        .unwrap();
    store.workspace_set_active_tab(ws, second.id, None).unwrap();

    let (_sub, heard) = recorded(&store);
    let dropped = store.tab_close(ws, second.id, None).unwrap();
    assert_eq!(dropped, vec![2]);
    let workspace = store.workspace(ws).unwrap();
    assert_eq!(workspace.tabs.len(), 1);
    assert_eq!(
        workspace.active_tab,
        Some(first.id),
        "the active tab may not dangle on a closed id"
    );
    assert!(
        matches!(
            heard.lock().unwrap().as_slice(),
            [
                (_, LayoutDelta::TabClosed { tab, reason: TabCloseReason::Explicit }),
                (_, LayoutDelta::ActiveTabChanged { tab: active })
            ] if *tab == second.id && *active == first.id
        ),
        "heard {:?}",
        heard.lock().unwrap()
    );

    heard.lock().unwrap().clear();
    let dropped = store.tab_close(ws, first.id, None).unwrap();
    assert_eq!(dropped, vec![1]);
    assert_eq!(
        store.workspace(ws).unwrap().active_tab,
        None,
        "a workspace with no tabs has no active one — the home-page state"
    );
    assert_eq!(
        heard.lock().unwrap().len(),
        1,
        "losing the last tab needs no ActiveTabChanged: no tabs, no active tab"
    );
}

#[test]
fn tabs_rename_move_and_regroup_in_place() {
    let (store, _dir, ws, first) = store_with_tab();
    let second = store
        .tab_create(ws, None, seed(2, "/b"), None, None)
        .unwrap();

    store
        .tab_rename(ws, first.id, Some("build".into()), None)
        .unwrap();
    let group = GroupId::new();
    store
        .tab_set_group(ws, first.id, Some(group), None, None)
        .unwrap();
    store.tab_move(ws, first.id, 1, None).unwrap();

    let workspace = store.workspace(ws).unwrap();
    assert_eq!(workspace.tabs[0].id, second.id);
    assert_eq!(workspace.tabs[1].name.as_deref(), Some("build"));
    assert_eq!(workspace.tabs[1].group, Some(group));
}

/// A tab put to sleep keeps its panes — ids in the layout, records in the
/// pane list — because waking is a restore of exactly those. Only the
/// orphan collection a close runs could take them, and a sleeping tab
/// still names them.
#[test]
fn a_sleeping_tab_keeps_its_panes_and_says_so_in_a_delta() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .pane_split(ws, 1, Axis::Horizontal, 0.5, seed(2, "/work"), false, None)
        .unwrap();
    let (_sub, heard) = recorded(&store);

    let panes = store.tab_set_hibernated(ws, tab.id, true, None).unwrap();

    assert_eq!(panes, vec![1, 2]);
    let m = store.machine();
    let asleep = &m.workspaces[0].tabs[0];
    assert!(asleep.hibernated);
    assert_eq!(asleep.root.pane_ids(), vec![1, 2]);
    assert!(m.panes.iter().any(|p| p.id == 1) && m.panes.iter().any(|p| p.id == 2));
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 1);
    match &heard[0].1 {
        LayoutDelta::TabRestructured { tab: t, pane: None } => {
            assert_eq!(t.id, tab.id);
            assert!(t.hibernated, "the delta carries the mark");
        }
        other => panic!("expected TabRestructured, heard {other:?}"),
    }
}

#[test]
fn setting_the_mark_a_tab_already_has_raises_nothing() {
    let (store, _dir, ws, tab) = store_with_tab();
    let (_sub, heard) = recorded(&store);
    store.tab_set_hibernated(ws, tab.id, false, None).unwrap();
    assert!(heard.lock().unwrap().is_empty());

    store.tab_set_hibernated(ws, tab.id, true, None).unwrap();
    store.tab_set_hibernated(ws, tab.id, true, None).unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1);

    store.tab_set_hibernated(ws, tab.id, false, None).unwrap();
    assert!(!store.machine().workspaces[0].tabs[0].hibernated);
    assert_eq!(heard.lock().unwrap().len(), 2);
}

#[test]
fn hibernating_a_tab_that_is_not_there_is_refused() {
    let (store, _dir, ws, _tab) = store_with_tab();
    let err = store
        .tab_set_hibernated(ws, TabId::new(), true, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

/// Asleep has to outlive the daemon: a restart reads the tree back from
/// disk, and a tab that came back awake would be respawned on the spot —
/// every process the user put to sleep, started again unasked.
#[test]
fn a_sleeping_tab_is_still_asleep_after_the_tree_is_read_back() {
    let (store, dir, ws, tab) = store_with_tab();
    let other = store
        .tab_create(ws, None, seed(5, "/else"), None, None)
        .unwrap();
    store.tab_set_hibernated(ws, tab.id, true, None).unwrap();
    drop(store);

    let reopened = MachineStore::open(dir.path().join(MACHINE_FILE));
    let m = reopened.machine();
    let tabs = &m.workspaces[0].tabs;
    assert!(tabs.iter().find(|t| t.id == tab.id).unwrap().hibernated);
    assert!(!tabs.iter().find(|t| t.id == other.id).unwrap().hibernated);
    let record = m.panes.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(record.cwd.as_deref(), Some("/work"));
    assert!(!record.live);
}

const NOON: u64 = 1_800_000_000;

/// A tab closed to be remembered leaves the workspace the way any close
/// takes it — other windows hear `TabClosed` — and lands on the list with
/// its layout and its panes' records, which is everything a reopen reads.
#[test]
fn a_remembered_close_keeps_the_tab_and_its_pane_records() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .pane_split(ws, 1, Axis::Horizontal, 0.5, seed(2, "/else"), false, None)
        .unwrap();
    assert!(store.machine().panes.iter().all(|p| p.live));
    let (_sub, heard) = recorded(&store);

    let closed = store
        .tab_close_remembered(ws, tab.id, &[1, 2], NOON, None)
        .unwrap();

    assert_eq!(closed.stopped, vec![1, 2]);
    assert!(closed.strays.is_empty() && closed.dropped.is_empty());
    let m = store.machine();
    assert!(m.workspaces[0].tabs.is_empty());
    let entry = &m.workspaces[0].closed[0];
    assert_eq!(entry.tab.id, tab.id);
    assert_eq!(entry.closed_at, NOON);
    assert_eq!(entry.tab.root.pane_ids(), vec![1, 2]);
    assert!(
        m.panes.iter().any(|p| p.id == 1) && m.panes.iter().any(|p| p.id == 2),
        "the records are what the reopen restores cwd and shell from"
    );
    assert!(
        m.panes.iter().all(|p| !p.live),
        "a stopped pane's record must not read as running to `lyrel wait`"
    );
    let heard = heard.lock().unwrap();
    assert!(
        matches!(&heard[0].1, LayoutDelta::TabClosed { tab: t, reason: TabCloseReason::Explicit } if *t == tab.id)
    );
}

/// A pane the window held but never got into the tree has no entry to
/// come back through, so the close ends it outright; one some other tab
/// still stands on is not the closing tab's to end at all.
#[test]
fn panes_the_tree_never_recorded_in_the_tab_are_strays() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .tab_create(ws, None, seed(5, "/other"), None, None)
        .unwrap();

    let closed = store
        .tab_close_remembered(ws, tab.id, &[1, 9, 5], NOON, None)
        .unwrap();

    assert_eq!(closed.stopped, vec![1]);
    assert_eq!(closed.strays, vec![9]);
}

/// Reopening hands back the newest entry with its records, and takes it
/// off the list; asking again walks back through the older ones, and an
/// empty list answers nothing rather than an error.
#[test]
fn reopening_takes_the_newest_closed_tab_off_the_list() {
    let (store, _dir, ws, first) = store_with_tab();
    let second = store
        .tab_create(ws, None, seed(2, "/second"), None, None)
        .unwrap();
    store
        .tab_close_remembered(ws, first.id, &[1], NOON, None)
        .unwrap();
    store
        .tab_close_remembered(ws, second.id, &[2], NOON + 5, None)
        .unwrap();

    let (reopened, dropped) = store.tab_reopen(ws, None, NOON + 10, None).unwrap();
    let reopened = reopened.expect("two tabs were closed");
    assert!(dropped.is_empty());
    assert_eq!(reopened.tab.id, second.id);
    assert_eq!(reopened.closed_at, NOON + 5);
    assert_eq!(reopened.panes.len(), 1);
    assert_eq!(reopened.panes[0].cwd.as_deref(), Some("/second"));

    let (older, _) = store.tab_reopen(ws, None, NOON + 10, None).unwrap();
    assert_eq!(older.unwrap().tab.id, first.id);
    let (none, _) = store.tab_reopen(ws, None, NOON + 10, None).unwrap();
    assert!(none.is_none());
    assert!(store.machine().workspaces[0].closed.is_empty());
}

/// The tab a reopen rebuilds is pushed as a new tab under its old id, and
/// a successor still connecting stands on its predecessor's id. The record
/// the reopen left behind is claimed, not refused — and a pane an open tab
/// really does hold still cannot be registered twice.
#[test]
fn a_reopened_tab_can_be_pushed_back_under_its_old_ids() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .tab_close_remembered(ws, tab.id, &[1], NOON, None)
        .unwrap();
    store.tab_reopen(ws, None, NOON, None).unwrap();

    let back = store
        .tab_create(ws, None, PaneSeed::bare(1), Some(tab.id), None)
        .unwrap();

    assert_eq!(back.id, tab.id);
    let m = store.machine();
    let record = m.panes.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(
        record.cwd.as_deref(),
        Some("/work"),
        "what the seed does not say, the kept record still does"
    );
    assert_eq!(m.panes.iter().filter(|p| p.id == 1).count(), 1);
    let twice = store.tab_create(ws, None, PaneSeed::bare(1), None, None);
    assert_eq!(twice.unwrap_err().kind(), io::ErrorKind::InvalidInput);
}

/// A closed tab's panes are the tree's until the entry goes: an ordinary
/// close elsewhere in the machine must not collect their records, or the
/// sweeps would take the screens the reopen is going to ask for.
#[test]
fn another_close_does_not_collect_a_remembered_tabs_records() {
    let (store, _dir, ws, tab) = store_with_tab();
    let other = store
        .tab_create(ws, None, seed(2, "/other"), None, None)
        .unwrap();
    store
        .tab_close_remembered(ws, tab.id, &[1], NOON, None)
        .unwrap();

    let orphans = store.tab_close(ws, other.id, None).unwrap();

    assert_eq!(orphans, vec![2]);
    assert!(store.machine().panes.iter().any(|p| p.id == 1));
}

/// Entries last a day. The one past it is dropped with its records and
/// answered, so its screen can be forgotten; the one inside it stays.
#[test]
fn closed_tabs_expire_after_a_day_with_their_records() {
    let (store, _dir, ws, old) = store_with_tab();
    let fresh = store
        .tab_create(ws, None, seed(2, "/fresh"), None, None)
        .unwrap();
    let day = CLOSED_TAB_TTL.as_secs();
    store
        .tab_close_remembered(ws, old.id, &[1], NOON, None)
        .unwrap();
    store
        .tab_close_remembered(ws, fresh.id, &[2], NOON + 60, None)
        .unwrap();

    assert!(store.expire_closed_tabs(NOON + day - 1).is_empty());
    assert_eq!(store.expire_closed_tabs(NOON + day), vec![1]);

    let m = store.machine();
    let left: Vec<TabId> = m.workspaces[0].closed.iter().map(|c| c.tab.id).collect();
    assert_eq!(left, vec![fresh.id]);
    assert!(!m.panes.iter().any(|p| p.id == 1));
    assert!(m.panes.iter().any(|p| p.id == 2));
    let (reopened, _) = store.tab_reopen(ws, None, NOON + 2 * day, None).unwrap();
    assert!(
        reopened.is_none(),
        "an entry past its day is never handed back, timer or no timer"
    );
}

/// A clock set back since a tab closed must not expire it on the spot.
#[test]
fn an_entry_stamped_in_the_future_is_kept() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .tab_close_remembered(ws, tab.id, &[1], NOON, None)
        .unwrap();
    assert!(store.expire_closed_tabs(NOON - 3600).is_empty());
    assert_eq!(store.machine().workspaces[0].closed.len(), 1);
}

/// The list holds the newest [`MAX_CLOSED_TABS`]; the close that makes it
/// one too long drops the oldest and answers its panes.
#[test]
fn the_list_keeps_only_the_newest_twenty() {
    let (store, _dir, ws, first) = store_with_tab();
    store
        .tab_close_remembered(ws, first.id, &[1], NOON, None)
        .unwrap();
    for n in 0..MAX_CLOSED_TABS as u64 {
        let pane = 100 + n;
        let tab = store
            .tab_create(ws, None, seed(pane, "/t"), None, None)
            .unwrap();
        let closed = store
            .tab_close_remembered(ws, tab.id, &[pane], NOON + 1 + n, None)
            .unwrap();
        let expect: Vec<u64> = match n + 1 == MAX_CLOSED_TABS as u64 {
            true => vec![1],
            false => Vec::new(),
        };
        assert_eq!(closed.dropped, expect, "after close {n}");
    }
    let m = store.machine();
    assert_eq!(m.workspaces[0].closed.len(), MAX_CLOSED_TABS);
    assert!(m.workspaces[0].closed.iter().all(|c| c.tab.id != first.id));
    assert!(!m.panes.iter().any(|p| p.id == 1));
}

/// The list is in the document, so it outlives the app and the daemon —
/// the whole point of keeping it here rather than in the window.
#[test]
fn closed_tabs_survive_the_tree_being_read_back() {
    let (store, dir, ws, tab) = store_with_tab();
    store.tab_set_hibernated(ws, tab.id, true, None).unwrap();
    store
        .tab_close_remembered(ws, tab.id, &[1], NOON, None)
        .unwrap();
    drop(store);

    let reopened = MachineStore::open(dir.path().join(MACHINE_FILE));
    let m = reopened.machine();
    let entry = &m.workspaces[0].closed[0];
    assert_eq!(entry.tab.id, tab.id);
    assert_eq!(entry.closed_at, NOON);
    assert!(!entry.tab.hibernated, "reopening is the wake");
    assert_eq!(
        m.panes.iter().find(|p| p.id == 1).unwrap().cwd.as_deref(),
        Some("/work")
    );
}
