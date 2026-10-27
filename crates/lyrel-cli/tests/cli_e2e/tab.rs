use super::*;

pub(super) fn tab_close_terminates_every_pane_in_the_tab(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let ws_id = created["id"].as_str().expect("new prints the workspace id");
    let tab = daemon.run_json(&["tab", "new", ws_id, "--cwd", &workdir()]);
    let tab_id = tab["tab"].as_str().expect("tab new prints the tab id");
    let first = tab["pane"].as_u64().expect("tab new prints the pane id");
    let first_addr = format!("%{first}");
    let split = daemon.run_json(&["split", &first_addr, "--horizontal"]);
    let second = split["pane"]
        .as_u64()
        .expect("split prints the new pane id");

    let tab_addr = format!("@{tab_id}");
    daemon.run_ok(&["tab", "close", &tab_addr]);

    let deadline = Instant::now() + CLOSE_WITHIN;
    loop {
        let listed = daemon.run_json(&["pane", "ls", "--all"]);
        let running = listed["panes"]
            .as_array()
            .expect("pane ls --all prints the daemon registry");
        let closed_are_gone = running
            .iter()
            .all(|pane| !matches!(pane["pane"].as_u64(), Some(id) if id == first || id == second));
        if closed_are_gone {
            assert_eq!(listed["orphans"].as_u64(), Some(0), "{listed}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "tab close left one of panes %{first} and %{second} live: {listed}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
