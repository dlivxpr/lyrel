use super::*;

const NOON: u64 = 1_800_000_000;

#[test]
fn a_sparse_document_decodes_with_defaults() {
    let machine: Machine =
        serde_json::from_str(r#"{"workspaces":[{"tabs":[{"root":{"Leaf":{"pane":3}}}]}]}"#)
            .expect("missing fields default rather than fail");
    assert_eq!(machine.workspaces.len(), 1);
    assert_eq!(machine.workspaces[0].tabs[0].root.pane_ids(), vec![3]);
    assert!(machine.panes.is_empty());
}

/// Awake is the default and stays out of the document, so a tree this
/// build writes for a tab that never slept reads the same to one that
/// predates the mark, and a tree from before it decodes as awake.
#[test]
fn the_mark_is_only_written_while_it_is_set() {
    let tab = Tab::leaf(4);
    let json = serde_json::to_string(&tab).unwrap();
    assert!(!json.contains("hibernated"), "{json}");
    let back: Tab = serde_json::from_str(&json).unwrap();
    assert!(!back.hibernated);

    let asleep = Tab {
        hibernated: true,
        ..Tab::leaf(4)
    };
    let back: Tab = serde_json::from_str(&serde_json::to_string(&asleep).unwrap()).unwrap();
    assert!(back.hibernated);
}

/// A tree written before the list existed has no `closed` key and reads as
/// an empty list; a workspace with nothing closed writes no key, so an
/// older build reads this build's tree unchanged.
#[test]
fn a_tree_without_the_list_loads_and_an_empty_list_is_not_written() {
    let old = r#"{"workspaces":[{"id":"6f1c0c0e-3b1a-4a8e-9d57-3c2b7c1f0a11","tabs":[]}],"panes":[]}"#;
    let m = serde_json::from_str::<Machine>(old).unwrap();
    assert!(m.workspaces[0].closed.is_empty());

    let json = serde_json::to_string(&Workspace::default()).unwrap();
    assert!(!json.contains("closed"), "{json}");

    let entry = ClosedTab {
        tab: Tab::leaf(3),
        closed_at: NOON,
    };
    let ws = Workspace {
        closed: vec![entry.clone()],
        ..Workspace::default()
    };
    let back: Workspace = serde_json::from_str(&serde_json::to_string(&ws).unwrap()).unwrap();
    assert_eq!(back.closed, vec![entry]);
    let stampless: ClosedTab =
        serde_json::from_str(r#"{"tab":{"root":{"Leaf":{"pane":3}}}}"#).unwrap();
    assert_eq!(stampless.closed_at, 0);
}
#[test]
fn unknown_agent_facts_do_not_discard_the_live_pane_or_shell() {
    let pane: PaneRecord = serde_json::from_value(serde_json::json!({
        "id": 19, "cwd": "/work", "title": "user job", "live": true,
        "shell": {"program":"bash", "args":["--login"]},
        "agent": {"agent":"FutureAgent", "session_id":"old"}
    }))
    .unwrap();
    assert_eq!(pane.id, 19);
    assert!(pane.live);
    assert_eq!(pane.cwd.as_deref(), Some("/work"));
    assert_eq!(pane.shell.as_ref().unwrap().program, "bash");
    assert!(pane.agent.is_none());
}
