use super::*;

pub(super) fn ls_on_an_empty_server(daemon: &Daemon) {
    let out = daemon.run_ok(&["ls"]);
    assert!(out.contains("no workspaces"), "{out}");
}

pub(super) fn new_builds_a_workspace_with_a_live_pane(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let ws_id = created["id"].as_str().expect("new prints the workspace id");
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    assert!(pane >= 1, "the daemon names panes from 1, got {pane}");

    let listed = daemon.run_json(&["ls"]);
    let workspaces = listed["workspaces"]
        .as_array()
        .expect("ls --json lists workspaces");
    assert_eq!(workspaces.len(), 1, "{listed}");
    assert_eq!(workspaces[0]["id"].as_str(), Some(ws_id), "{listed}");
    assert_eq!(workspaces[0]["panes"].as_u64(), Some(1), "{listed}");

    let panes = daemon.run_ok(&["pane", "ls"]);
    assert!(panes.contains(&format!("%{pane}")), "{panes}");
}

/// A pane's owner is the workspace allowed to attach to it, and the GUI
/// respawns over anything else — so every way the CLI makes a pane has to
/// stamp that id, not a name of its own. It used to write a literal
/// "lyrel-cli", which left a CLI-built workspace rebuilt from scratch the
/// first time a window opened on it: fresh shells, the live ones orphaned.
pub(super) fn every_pane_the_cli_files_names_its_workspace_as_owner(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let ws_id = created["id"]
        .as_str()
        .expect("new prints the workspace id")
        .to_string();

    let tab = daemon.run_json(&["tab", "new", &ws_id, "--cwd", &workdir()]);
    let tabbed = tab["pane"].as_u64().expect("tab new prints the pane id");
    daemon.run_json(&["split", &format!("%{tabbed}"), "--horizontal"]);

    let listed = daemon.run_json(&["pane", "ls", "--all"]);
    let panes = listed["panes"]
        .as_array()
        .expect("pane ls --all prints the daemon registry");
    let ours: Vec<&serde_json::Value> = panes
        .iter()
        .filter(|p| p["workspace"].as_str() == Some(ws_id.as_str()))
        .collect();
    // `run --keep` files a pane the same way, but its command has to exit for
    // the CLI to return, and the registry drops the pane with it — so the
    // three that outlive their command are what can be read back here.
    assert_eq!(
        ours.len(),
        3,
        "new, tab new and split each filed one pane: {listed}"
    );
    for pane in ours {
        assert_eq!(
            pane["owner"].as_str(),
            Some(ws_id.as_str()),
            "a pane its workspace holds must name that workspace as owner: {pane}"
        );
    }
}
