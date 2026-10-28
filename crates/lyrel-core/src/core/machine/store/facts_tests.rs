use super::super::test_support::*;
use super::*;

#[test]
fn pane_facts_update_the_record_and_reach_every_client() {
    let (store, _dir, ws, _tab) = store_with_tab();
    let (sub, heard) = recorded(&store);

    store.note_pane_facts(1, |p| {
        p.cwd = Some("/work/deeper".into());
    });
    let record = store.pane(1).unwrap();
    assert_eq!(record.cwd.as_deref(), Some("/work/deeper"));
    {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].0, ws.to_string());
        assert!(matches!(&heard[0].1, LayoutDelta::PaneFacts { pane } if pane.id == 1));
    }

    store.note_pane_facts(1, |_| {});
    store.note_pane_facts(999, |p| p.cwd = Some("/ghost".into()));
    assert_eq!(heard.lock().unwrap().len(), 1);
    drop(sub);
}

#[test]
fn published_observations_land_in_the_installed_store() {
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    observe_pane(1, |p| p.cwd = Some("/nowhere".into()));

    let (store, _dir, _ws, _tab) = store_with_tab();
    publish_observations(&store);
    observe_pane(1, |p| p.cwd = Some("/observed/here".into()));
    assert_eq!(
        store.pane(1).unwrap().cwd.as_deref(),
        Some("/observed/here")
    );
    withdraw_observations();
}

#[test]
fn an_observation_is_broadcast_at_once_and_written_a_little_later() {
    let (store, dir, ws, _tab) = store_with_tab();
    let path = dir.path().join(MACHINE_FILE);
    let (_sub, heard) = recorded(&store);

    store.note_pane_facts(1, |p| p.cwd = Some("/work/deeper".into()));
    assert_eq!(
        heard.lock().unwrap().len(),
        1,
        "the client hears the fact immediately; only the disk waits"
    );
    assert!(
        !std::fs::read_to_string(&path).unwrap().contains("deeper"),
        "an observation must not write the document synchronously"
    );

    store.flush();
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("deeper"),
        "…and the flush is what puts it on disk"
    );
    let before = std::fs::metadata(&path).unwrap().len();
    store.flush();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before);

    store.note_pane_facts(1, |p| p.cwd = Some("/work/deepest".into()));
    store
        .workspace_rename(ws, Some("web".into()), None)
        .unwrap();
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("deepest"),
        "a structural write carries whatever the facts left unwritten"
    );
}
