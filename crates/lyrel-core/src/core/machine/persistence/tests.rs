use super::super::test_support::*;
use super::*;
use crate::core::machine::{Axis, MachineStore, PaneNode, Tab, Workspace};

#[test]
fn the_tree_round_trips_through_the_file() {
    let (store, dir) = store();
    let ws = store
        .workspace_create(None, Some("api".into()), None)
        .unwrap();
    store
        .tab_create(ws.id, None, seed(1, "/work"), None, None)
        .unwrap();
    store
        .pane_split(
            ws.id,
            1,
            Axis::Vertical,
            0.3,
            seed(2, "/work/api"),
            false,
            None,
        )
        .unwrap();

    let reopened = MachineStore::open(dir.path().join(MACHINE_FILE));
    let machine = reopened.machine();
    assert_eq!(machine.workspaces.len(), 1);
    let back = &machine.workspaces[0];
    assert_eq!(back.id, ws.id, "workspace identity survives a restart");
    assert_eq!(back.name.as_deref(), Some("api"));
    assert_eq!(back.tabs.len(), 1);
    assert_eq!(back.tabs[0].root.pane_ids(), vec![1, 2]);
    match &back.tabs[0].root {
        PaneNode::Split { axis, ratio, .. } => {
            assert_eq!(*axis, Axis::Vertical);
            assert!((ratio - 0.3).abs() < 1e-6);
        }
        PaneNode::Leaf { .. } => panic!("the split has to survive"),
    }
    assert_eq!(
        machine.panes.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![1, 2],
        "the pane registry rides the same file"
    );
    assert_eq!(machine.panes[0].cwd.as_deref(), Some("/work"));
}

#[test]
fn a_reopened_store_marks_every_pane_awaiting_revival() {
    let (store, dir) = store();
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(ws.id, None, seed(7, "/work"), None, None)
        .unwrap();
    assert!(
        store.pane(7).unwrap().live,
        "the pane its own client just seeded is live"
    );

    let restarted = MachineStore::open(dir.path().join(MACHINE_FILE));
    let record = restarted.pane(7).expect("the record survives the restart");
    assert!(!record.live, "a restarted daemon has no live panes");
    assert_eq!(
        record.cwd.as_deref(),
        Some("/work"),
        "the facts a successor spawns from survive"
    );
    assert_eq!(
        restarted.workspace(ws.id).unwrap().tabs[0].root.pane_ids(),
        vec![7],
        "the leaf still names the dead pane: that is the revival slot"
    );
}

#[test]
fn a_corrupt_file_is_quarantined_rather_than_overwritten() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, b"{ this is not json").unwrap();

    let store = MachineStore::open(&path);
    assert!(store.machine().workspaces.is_empty());
    store.workspace_create(None, None, None).unwrap();
    let aside = std::fs::read_to_string(path.with_extension("json.corrupt")).unwrap();
    assert_eq!(aside, "{ this is not json");

    std::fs::write(&path, b"corrupt again").unwrap();
    let store = MachineStore::open(&path);
    store.workspace_create(None, None, None).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.with_extension("json.corrupt")).unwrap(),
        "{ this is not json",
        "the first rescue copy is still the first one"
    );
    assert_eq!(
        std::fs::read_to_string(path.with_extension("json.corrupt.1")).unwrap(),
        "corrupt again"
    );
}

#[cfg(unix)]
#[test]
fn the_document_is_written_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let (store, dir) = store();
    store.workspace_create(None, None, None).unwrap();
    let mode = std::fs::metadata(dir.path().join(MACHINE_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_moved_aside_rather_than_overwritten() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, b"{\"workspaces\":[]}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_to_string(&path).is_ok() {
        return;
    }

    let store = MachineStore::open(&path);
    store.workspace_create(None, None, None).unwrap();

    let aside = path.with_extension("json.corrupt");
    assert!(aside.exists(), "the unreadable original must be kept");
    std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&aside).unwrap(),
        "{\"workspaces\":[]}",
        "moved aside byte-for-byte, ready for a hand repair"
    );
}

/// One tree with `tabs` tabs in it, written out the way `persist` writes.
fn document(tabs: u64) -> Vec<u8> {
    let machine = Machine {
        workspaces: vec![Workspace {
            tabs: (0..tabs).map(Tab::leaf).collect(),
            ..Workspace::default()
        }],
        panes: Vec::new(),
    };
    serde_json::to_vec_pretty(&machine).unwrap()
}

fn tabs_in(path: &Path) -> usize {
    let text = std::fs::read_to_string(path).expect("a generation must be there to read");
    parse_machine(&text).expect("and it must parse").workspaces[0]
        .tabs
        .len()
}

/// A machine tree is written whole on every mutation, so the write that
/// loses the layout also erases the only copy of it (#716). One generation
/// back is the least that makes a bad write survivable by hand.
#[test]
fn the_document_being_replaced_is_kept_beside_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    let store = MachineStore::open(&path);

    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(ws.id, None, seed(1, "/work"), None, None)
        .unwrap();
    store
        .tab_create(ws.id, None, seed(2, "/work"), None, None)
        .unwrap();

    let kept = backup_path(&path, 0);
    assert!(
        kept.exists(),
        "the second write has a first write to keep: {}",
        kept.display()
    );
    assert_eq!(
        tabs_in(&kept),
        0,
        "and what it kept is the document that write replaced"
    );
}

/// The whole reason the generations are spaced. The failure this came from
/// arrived as a burst — nineteen `TabClose`s, nineteen persists, seconds
/// apart — and a ring that rotated on every write would have held three
/// copies of the damage by the time anyone looked at it.
#[test]
fn a_burst_of_writes_cannot_roll_the_history_away() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, document(19)).unwrap();
    keep_a_generation(&path, Duration::ZERO).unwrap();

    for remaining in (0..19).rev() {
        std::fs::write(&path, document(remaining)).unwrap();
        keep_a_generation(&path, BACKUP_SPACING).unwrap();
    }

    assert_eq!(tabs_in(&path), 0, "the burst emptied the live tree");
    assert_eq!(
        tabs_in(&backup_path(&path, 0)),
        19,
        "and the kept generation is from before it, not from one write ago"
    );
}

#[test]
fn the_ring_keeps_three_generations_and_no_more() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);

    for tabs in 1..=6 {
        std::fs::write(&path, document(tabs)).unwrap();
        keep_a_generation(&path, Duration::ZERO).unwrap();
    }

    assert_eq!(tabs_in(&backup_path(&path, 0)), 6);
    assert_eq!(tabs_in(&backup_path(&path, 1)), 5);
    assert_eq!(tabs_in(&backup_path(&path, 2)), 4);
    assert!(
        !backup_path(&path, 3).exists(),
        "the ring is bounded; a tree rewritten every few seconds must not \
         grow a file per write forever"
    );
}

/// The automatic half of the recovery. A tree that parses always wins —
/// including an empty one, which is why the manual `.bak` copy still
/// matters — but a tree that does not parse used to mean starting over.
#[test]
fn a_tree_that_does_not_parse_is_loaded_from_the_newest_backup() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, document(3)).unwrap();
    keep_a_generation(&path, Duration::ZERO).unwrap();
    std::fs::write(&path, b"{ not json").unwrap();

    let store = MachineStore::open(&path);

    assert_eq!(
        store.machine().workspaces[0].tabs.len(),
        3,
        "the layout comes back from the kept generation"
    );
    assert!(
        path.with_extension("json.corrupt").exists(),
        "and the unparseable document is still kept for inspection"
    );
}
