use super::super::test_support::*;
use super::*;
use crate::core::machine::{Axis, MACHINE_FILE, Side, TabId};

#[test]
fn a_refused_operation_changes_nothing_and_notifies_nobody() {
    let (store, _dir, ws, tab) = store_with_tab();
    let before = store.machine();
    let (_sub, heard) = recorded(&store);

    let missing = WorkspaceId::new();
    assert!(store.workspace_rename(missing, None, None).is_err());
    assert!(
        store
            .tab_create(missing, None, seed(9, "/x"), None, None)
            .is_err()
    );
    assert!(store.tab_close(ws, TabId::new(), None).is_err());
    assert!(
        store
            .pane_split(ws, 999, Axis::Vertical, 0.5, seed(9, "/x"), false, None)
            .is_err()
    );
    assert!(store.pane_close(ws, 999, None).is_err());
    assert!(
        store
            .pane_set_ratio(ws, tab.id, vec![Side::A], 0.5, None)
            .is_err()
    );
    assert!(
        store
            .pane_set_ratio(ws, tab.id, vec![], f32::NAN, None)
            .is_err()
    );
    assert!(store.pane_replace(ws, 999, seed(9, "/x"), None).is_err());

    assert_eq!(store.machine(), before);
    assert!(heard.lock().unwrap().is_empty());
    assert!(
        store.pane(9).is_none(),
        "a seed on a refused op must not leak into the registry"
    );
}

#[test]
fn a_delta_reaches_every_subscriber_but_its_author() {
    let (store, _dir, ws, _tab) = store_with_tab();
    let (author, heard_by_author) = recorded(&store);
    let (_other, heard_by_other) = recorded(&store);

    store
        .workspace_rename(ws, Some("renamed".into()), Some(author.id()))
        .unwrap();
    assert!(heard_by_author.lock().unwrap().is_empty());
    assert_eq!(heard_by_other.lock().unwrap().len(), 1);

    store.workspace_rename(ws, None, None).unwrap();
    assert_eq!(heard_by_author.lock().unwrap().len(), 1);
    assert_eq!(heard_by_other.lock().unwrap().len(), 2);
}

#[test]
fn dropping_a_subscription_stops_the_deltas() {
    let (store, _dir, ws, _tab) = store_with_tab();
    let (sub, heard) = recorded(&store);
    store.workspace_touch(ws, None).unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1);
    drop(sub);
    store.workspace_touch(ws, None).unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1);
}

#[test]
fn a_structural_edit_is_on_disk_before_anyone_hears_about_it() {
    let (store, dir) = store();
    let path = dir.path().join(MACHINE_FILE);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let path_in_callback = path.clone();
    let _sub = store.subscribe(Arc::new(move |_ws: &str, _delta: &LayoutDelta| {
        sink.lock()
            .unwrap()
            .push(std::fs::read_to_string(&path_in_callback).unwrap_or_default());
    }));

    let ws = store
        .workspace_create(None, Some("api".into()), None)
        .unwrap();
    let _ = ws;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].contains("api"),
        "the delta arrived before the file said so: {}",
        seen[0]
    );
}

#[test]
fn a_failed_structural_write_rolls_back_before_publication() {
    let (store, dir, ws, _tab) = store_with_tab();
    let before = store.machine();
    let (_sub, heard) = recorded(&store);
    let path = dir.path().join(MACHINE_FILE);
    let held = dir.path().join("accepted.json");
    std::fs::rename(&path, &held).unwrap();
    std::fs::create_dir(&path).unwrap();

    let failed = store.tab_create(ws, None, seed(9, "/rejected"), None, None);
    assert!(
        failed.is_err(),
        "a directory cannot accept the atomic document rename"
    );
    assert_eq!(
        store.machine(),
        before,
        "the failed seed and tab must both roll back"
    );
    assert!(
        heard.lock().unwrap().is_empty(),
        "a rejected operation has no delta"
    );

    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&held, &path).unwrap();
    let accepted = store
        .tab_create(ws, None, seed(9, "/accepted"), None, None)
        .unwrap();
    let reopened = MachineStore::open(&path);
    assert_eq!(
        reopened.workspace(ws).unwrap().tabs.last().unwrap().id,
        accepted.id
    );
    assert_eq!(
        heard.lock().unwrap().len(),
        2,
        "the accepted tab and active-tab deltas publish normally"
    );
}
