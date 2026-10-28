use std::sync::{Arc, Mutex};

use crate::core::session::WorkspaceId;

use super::{LayoutDelta, MACHINE_FILE, MachineStore, PaneSeed, Subscription, Tab};

pub(super) fn store() -> (Arc<MachineStore>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    (MachineStore::open(dir.path().join(MACHINE_FILE)), dir)
}

pub(super) fn seed(pane: u64, cwd: &str) -> PaneSeed {
    PaneSeed {
        pane,
        cwd: Some(cwd.to_string()),
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

pub(super) fn store_with_tab() -> (Arc<MachineStore>, tempfile::TempDir, WorkspaceId, Tab) {
    let (store, dir) = store();
    let ws = store
        .workspace_create(None, Some("api".into()), None)
        .unwrap();
    let tab = store
        .tab_create(ws.id, None, seed(1, "/work"), None, None)
        .unwrap();
    (store, dir, ws.id, tab)
}

pub(super) fn recorded(
    store: &Arc<MachineStore>,
) -> (Subscription, Arc<Mutex<Vec<(String, LayoutDelta)>>>) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&heard);
    let sub = store.subscribe(Arc::new(move |ws: &str, delta: &LayoutDelta| {
        sink.lock().unwrap().push((ws.to_string(), delta.clone()));
    }));
    (sub, heard)
}
