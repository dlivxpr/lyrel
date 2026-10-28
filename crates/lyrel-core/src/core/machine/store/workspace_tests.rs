use super::super::test_support::*;
use super::*;
use crate::core::group_key::WorkspaceGroups;

#[test]
fn a_client_minted_workspace_id_is_kept_and_a_duplicate_is_refused() {
    let (store, _dir) = store();
    let id = WorkspaceId::new();
    let ws = store
        .workspace_create(Some(id), Some("api".into()), None)
        .unwrap();
    assert_eq!(ws.id, id, "the id the client named is the id it gets");

    let refused = store
        .workspace_create(Some(id), None, None)
        .expect_err("a second create on the same id must refuse");
    assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(
        store.machine().workspaces.len(),
        1,
        "the refusal changed nothing"
    );
}

#[test]
fn workspace_create_rename_touch_delete_land_and_broadcast() {
    let (store, _dir) = store();
    let (_sub, heard) = recorded(&store);

    let ws = store
        .workspace_create(None, Some("api".into()), None)
        .unwrap();
    store
        .workspace_rename(ws.id, Some("web".into()), None)
        .unwrap();
    store.workspace_touch(ws.id, None).unwrap();
    assert_eq!(store.workspace(ws.id).unwrap().name.as_deref(), Some("web"));

    store.workspace_delete(ws.id, None).unwrap();
    assert!(store.workspace(ws.id).is_err());

    let heard = heard.lock().unwrap();
    let kinds: Vec<&LayoutDelta> = heard.iter().map(|(_, d)| d).collect();
    assert!(matches!(kinds[0], LayoutDelta::WorkspaceCreated { .. }));
    assert!(matches!(kinds[1], LayoutDelta::WorkspaceRenamed { name: Some(n) } if n == "web"));
    assert!(matches!(kinds[2], LayoutDelta::WorkspaceTouched { .. }));
    assert!(matches!(kinds[3], LayoutDelta::WorkspaceDeleted));
    assert!(
        heard.iter().all(|(key, _)| key == &ws.id.to_string()),
        "every delta names the workspace it is about"
    );
}

#[test]
fn deleting_a_workspace_forgets_the_panes_only_it_referenced() {
    let (store, _dir, ws, _tab) = store_with_tab();
    let dropped = store.workspace_delete(ws, None).unwrap();
    assert_eq!(dropped, vec![1], "the caller is told which PTYs to kill");
    assert!(store.pane(1).is_none());
}

/// Dropping a group hands its tabs back to auto grouping in the same
/// mutation, and says so — a window that only heard `GroupsChanged` would
/// otherwise keep a tab filed under a group it can no longer draw.
#[test]
fn dropping_a_group_returns_its_tabs_to_auto_grouping() {
    use crate::core::group_key::PinnedGroup;
    let (store, _dir, ws, first) = store_with_tab();
    let second = store
        .tab_create(ws, None, seed(2, "/b"), None, None)
        .unwrap();
    let (keep, drop) = (PinnedGroup::label("keep"), PinnedGroup::label("drop"));
    let groups = WorkspaceGroups {
        pinned: vec![keep.clone(), drop.clone()],
        ..Default::default()
    };
    store.workspace_set_groups(ws, groups, None).unwrap();
    store
        .tab_set_group(ws, first.id, Some(drop.id), None, None)
        .unwrap();
    store
        .tab_set_group(ws, second.id, Some(keep.id), None, None)
        .unwrap();

    let (_sub, heard) = recorded(&store);
    let kept = WorkspaceGroups {
        pinned: vec![keep.clone()],
        ..Default::default()
    };
    store.workspace_set_groups(ws, kept.clone(), None).unwrap();

    let workspace = store.workspace(ws).unwrap();
    assert_eq!(workspace.groups, kept);
    assert_eq!(workspace.tabs[0].group, None, "back to auto grouping");
    assert_eq!(workspace.tabs[1].group, Some(keep.id), "untouched");
    let heard = heard.lock().unwrap();
    assert_eq!(
        heard.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        vec![
            LayoutDelta::TabRegrouped {
                tab: first.id,
                group: None,
                last_auto: None,
            },
            LayoutDelta::GroupsChanged { groups: kept },
        ]
    );
}

#[test]
fn attachments_takeover_and_are_never_persisted() {
    let (store, dir, ws, _tab) = store_with_tab();
    assert_eq!(store.attachment(ws), None);

    let laptop = Attachment::new("tok-1", "laptop");
    assert_eq!(store.attach(ws, laptop.clone()), None);
    let desktop = Attachment::new("tok-2", "desktop");
    assert_eq!(store.attach(ws, desktop.clone()), Some(laptop.clone()));

    assert!(!store.detach(ws, &laptop.token));
    assert_eq!(store.attachment(ws).unwrap().hostname, "desktop");
    assert!(store.detach(ws, &desktop.token));
    assert_eq!(store.attachment(ws), None);

    store.attach(ws, Attachment::new("secret-token", "laptop"));
    store
        .workspace_rename(ws, Some("web".into()), None)
        .unwrap();
    let text = std::fs::read_to_string(dir.path().join(MACHINE_FILE)).unwrap();
    assert!(!text.contains("secret-token"), "{text}");
    assert_eq!(
        MachineStore::open(dir.path().join(MACHINE_FILE)).attachment(ws),
        None
    );
}

#[test]
fn an_attachment_travels_by_name_and_never_by_token() {
    let (store, _dir, ws, _tab) = store_with_tab();
    store.attach(ws, Attachment::new("secret-token", "laptop"));

    // What `MachineGet` hands a peer: `lyrel ls` reads its ATTACHED column
    // out of this, so a held workspace has to say so here.
    let wire = serde_json::to_string(&store.machine()).unwrap();
    assert!(wire.contains("laptop"), "{wire}");
    assert!(!wire.contains("secret-token"), "{wire}");

    let seen: Machine = serde_json::from_str(&wire).unwrap();
    let held = seen.workspaces.iter().find(|w| w.id == ws).unwrap();
    assert_eq!(held.attachment.as_ref().unwrap().hostname, "laptop");
    assert!(held.attachment.as_ref().unwrap().token.is_empty());
}

#[test]
fn an_attachment_dies_with_its_workspace() {
    let (store, _dir, ws, _tab) = store_with_tab();
    store.attach(ws, Attachment::new("tok", "laptop"));
    assert!(store.attachment(ws).is_some());
    store.workspace_delete(ws, None).unwrap();
    assert_eq!(store.attachment(ws), None);
}
