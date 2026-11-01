use super::*;

pub(super) fn repeated_pane_pressure_preserves_detached_history_and_closes_only_owned_panes(
    daemon: &Daemon,
) {
    use lyrel_core::core::machine::PaneSeed;
    use lyrel_core::daemon::control::ControlRequest;

    let workspace = daemon.run_json(&["ws", "new", "lifecycle-retention"]);
    let workspace = workspace["id"].as_str().unwrap();
    let workspace_id = workspace.parse().unwrap();
    let control = ControlClient::connect_at(
        &daemon.control_endpoint(),
        &ControlHello::host_rpc("lifecycle-retention", "lifecycle-retention"),
    )
    .unwrap();
    let panes = PaneClient::at(daemon.pane_endpoint());
    let size = WinSize {
        cols: 100,
        rows: 30,
        cell_w: 8,
        cell_h: 16,
    };
    let mut live = Vec::new();
    for _ in 0..2 {
        for count in [1, 16, 1, 32, 1] {
            while live.len() < count {
                let mut session = panes
                    .spawn(
                        None,
                        size,
                        Some(ShellSpec {
                            program: std::env::current_exe().unwrap().display().to_string(),
                            args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
                            args_are_lyrel_defaults: false,
                        }),
                        Some(workspace.to_string()),
                        None,
                    )
                    .unwrap();
                session.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
                collect_pane_output_until(&mut session, b"lyrel_e2e_tail_line_6\r\n");
                let pane = session.pane_id();
                control
                    .request(ControlRequest::TabCreate {
                        workspace: workspace_id,
                        at: None,
                        pane: PaneSeed::bare(pane),
                        tab: None,
                    })
                    .unwrap();
                session.detach().unwrap();
                live.push(pane);
            }
            for pane in live.drain(count..) {
                daemon.run_ok(&["pane", "close", &format!("%{pane}")]);
                assert!(
                    panes.attach(pane, size).is_err(),
                    "an explicitly closed pane must not be recoverable as a live session"
                );
            }
            let listed = daemon.run_json(&["pane", "ls", "--all"]);
            let entries = listed["panes"].as_array().unwrap();
            assert_eq!(entries.len(), count, "{listed}");
            assert_eq!(listed["orphans"].as_u64(), Some(0), "{listed}");
            for pane in &live {
                assert!(
                    entries.iter().any(|entry| {
                        entry["pane"].as_u64() == Some(*pane) && entry["live"] == true
                    }),
                    "a detached session lost its live owner: {listed}"
                );
            }
            let captured = daemon.run_json(&[
                "capture",
                &format!("%{}", live[0]),
                "--plain",
                "--scrollback",
            ]);
            let text = captured["text"].as_str().unwrap();
            for line in ["lyrel_e2e_tail_line_1", "lyrel_e2e_tail_line_6"] {
                assert!(text.contains(line), "detached history was lost: {captured}");
            }
        }
    }
    daemon.run_ok(&["ws", "rm", workspace]);
    let listed = daemon.run_json(&["pane", "ls", "--all"]);
    assert!(listed["panes"].as_array().unwrap().is_empty(), "{listed}");
    assert_eq!(listed["orphans"].as_u64(), Some(0), "{listed}");
}

pub(super) fn retired_executable_names_still_run_as_ordinary_pty_commands(daemon: &Daemon) {
    // An ordinary existing runner fixture, not an agent protocol mock or real agent evidence.
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(ShellSpec {
                program: executable.display().to_string(),
                args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
                args_are_lyrel_defaults: false,
            }),
            Some("ordinary-retired-name-e2e".into()),
            None,
        )
        .expect("the supported-agent list is not an executable allowlist");
    pane.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
    collect_pane_output_until(&mut pane, b"lyrel_e2e_tail_line_6\r\n");
    let address = format!("%{}", pane.pane_id());
    let captured = daemon.run_ok(&["capture", &address, "--scrollback"]);
    assert!(captured.contains("lyrel_e2e_tail_line_6"), "{captured}");
    let procs = daemon.run_json(&["procs", &address]);
    assert!(
        procs["procs"].as_array().unwrap().iter().any(|p| p["name"]
            .as_str()
            .is_some_and(|name| name.contains("claude"))),
        "the ordinary child must still run: {procs}"
    );
    daemon.run_ok(&["send", &address, "finish", "--enter"]);
    loop {
        match pane.recv().expect("the fixture must exit after its input") {
            DaemonMsg::Exited { code } => {
                assert_eq!(code, Some(0));
                break;
            }
            _ => {}
        }
    }
}

pub(super) fn parked_shells_keep_cli_identity_input_capture_and_close_scope(daemon: &Daemon) {
    use lyrel_core::core::machine::{Axis, PaneSeed};
    use lyrel_core::daemon::control::{ControlRequest, ReplyOk};

    let client = ControlClient::connect_at(
        &daemon.control_endpoint(),
        &ControlHello::host_rpc("parking-cli", "parking-cli"),
    )
    .unwrap();
    let ws = match client
        .request(ControlRequest::WorkspaceCreate {
            name: Some("parked-cli".into()),
            workspace: None,
        })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(workspace) => workspace.id,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    let panes = PaneClient::at(daemon.pane_endpoint());
    let shell = ShellSpec {
        program: if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into(),
        args: if cfg!(windows) {
            vec!["/d".into()]
        } else {
            Vec::new()
        },
        args_are_lyrel_defaults: false,
    };
    let sessions = (0..2)
        .map(|_| {
            panes
                .spawn(
                    None,
                    WinSize {
                        cols: 100,
                        rows: 30,
                        cell_w: 8,
                        cell_h: 16,
                    },
                    Some(shell.clone()),
                    Some(ws.to_string()),
                    None,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (a, b) = (sessions[0].pane_id(), sessions[1].pane_id());
    let seed = |pane| PaneSeed {
        pane,
        cwd: Some(workdir()),
        ssh_spec: None,
        agent: None,
        shell: None,
    };
    client
        .request(ControlRequest::TabCreate {
            workspace: ws,
            at: None,
            pane: seed(a),
            tab: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneSplit {
            workspace: ws,
            pane: a,
            axis: Axis::Horizontal,
            ratio: 0.5,
            new: seed(b),
            first: false,
        })
        .unwrap();
    let a_addr = format!("%{a}");
    let b_addr = format!("%{b}");
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "set lyrel_park_cli_state=retained"
        } else {
            "lyrel_park_cli_state=retained"
        },
        "--enter",
    ]);
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "echo BEFORE_%lyrel_park_cli_state%"
        } else {
            "printf 'BEFORE_%s\\n' \"$lyrel_park_cli_state\""
        },
        "--enter",
    ]);
    await_capture(daemon, &a_addr, "BEFORE_retained");
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: a,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PaneSetPresentation {
            workspace: ws,
            pane: b,
            pip: true,
            fallback: None,
            restore: None,
        })
        .unwrap();
    client
        .request(ControlRequest::PanePark {
            workspace: ws,
            pane: b,
            restore: None,
        })
        .unwrap();
    let tree = match client
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    assert_eq!(tree.tabs.len(), 1);
    assert!(tree.tabs[0].visible_root().is_none());
    assert!(tree.tabs[0].presentation.pip().is_none());
    let listed = daemon.run_json(&["pane", "ls", "--all"]);
    let registry = listed["panes"].as_array().unwrap();
    assert_eq!(
        registry.len(),
        2,
        "all parked must not create a filler shell: {listed}"
    );
    for pane in [a, b] {
        let entry = registry
            .iter()
            .find(|entry| entry["pane"].as_u64() == Some(pane))
            .unwrap();
        assert_eq!(
            entry["owner"].as_str(),
            Some(ws.to_string().as_str()),
            "{listed}"
        );
        assert_eq!(
            entry["workspace"].as_str(),
            Some(ws.to_string().as_str()),
            "{listed}"
        );
    }
    assert_eq!(
        listed["orphans"].as_u64(),
        Some(0),
        "parked sessions retain ownership: {listed}"
    );
    let workspaces = daemon.run_json(&["ls"]);
    assert_eq!(
        workspaces["workspaces"][0]["panes"].as_u64(),
        Some(2),
        "{workspaces}"
    );
    let tab_list = daemon.run_json(&["tab", "ls", &ws.to_string()]);
    assert_eq!(tab_list["tabs"].as_array().unwrap().len(), 1, "{tab_list}");
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "echo PARKED_%lyrel_park_cli_state%"
        } else {
            "printf 'PARKED_%s\\n' \"$lyrel_park_cli_state\""
        },
        "--enter",
    ]);
    await_capture(daemon, &a_addr, "PARKED_retained");
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: b,
            fallback: None,
        })
        .unwrap();
    let chosen = match client
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    assert_eq!(chosen.tabs[0].visible_root().unwrap().pane_ids(), vec![b]);
    assert!(chosen.tabs[0].presentation.is_parked(a));
    assert!(chosen.tabs[0].presentation.pip().is_none());
    client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: Some(b),
        })
        .unwrap();
    daemon.run_ok(&[
        "send",
        &a_addr,
        if cfg!(windows) {
            "echo RESTORED_%lyrel_park_cli_state%"
        } else {
            "printf 'RESTORED_%s\\n' \"$lyrel_park_cli_state\""
        },
        "--enter",
    ]);
    await_capture(daemon, &a_addr, "RESTORED_retained");
    for pane in [a, b] {
        client
            .request(ControlRequest::PanePark {
                workspace: ws,
                pane,
                restore: None,
            })
            .unwrap();
    }
    daemon.run_ok(&["pane", "close", &a_addr]);
    let stale = client
        .request(ControlRequest::PaneRestore {
            workspace: ws,
            pane: a,
            fallback: Some(b),
        })
        .unwrap_err();
    assert_eq!(stale.kind(), std::io::ErrorKind::NotFound);
    let deadline = Instant::now() + CLOSE_WITHIN;
    loop {
        let listed = daemon.run_json(&["pane", "ls", "--all"]);
        let registry = listed["panes"].as_array().unwrap();
        assert!(
            registry
                .iter()
                .any(|entry| entry["pane"].as_u64() == Some(b)),
            "closing one parked pane must retain the other: {listed}"
        );
        if !registry
            .iter()
            .any(|entry| entry["pane"].as_u64() == Some(a))
        {
            assert_eq!(
                registry.len(),
                1,
                "stale restore must not create a session: {listed}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "closed parked pane is still live: {listed}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let after_close = match client
        .request(ControlRequest::WorkspaceTree { workspace: ws })
        .unwrap()
    {
        ReplyOk::WorkspaceTree(tree) => tree,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    assert!(after_close.tabs[0].visible_root().is_none());
    assert!(after_close.tabs[0].presentation.is_parked(b));
    daemon.run_ok(&[
        "send",
        &b_addr,
        if cfg!(windows) {
            "echo SURVIVOR_%LYREL_PANE%"
        } else {
            "printf 'SURVIVOR_%s\\n' \"$LYREL_PANE\""
        },
        "--enter",
    ]);
    await_capture(daemon, &b_addr, &format!("SURVIVOR_{b}"));
    daemon.run_ok(&["pane", "close", &b_addr]);
    drop(sessions);
}

/// `wait --until free` reads freeness off this object, and its whole point is
/// that a process list alone cannot say whether it covers the pane. A real
/// daemon, a real pty: the context has to come back filled, and say this
/// machine holds the pane (#840).
pub(super) fn procs_says_where_the_panes_session_lives(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");

    let procs = daemon.run_json(&["procs", &format!("%{pane}")]);
    let context = &procs["context"];
    assert!(
        context.is_object(),
        "a current server always answers with a context: {procs}"
    );
    assert_eq!(
        context["local_pty"].as_bool(),
        Some(true),
        "a pane this daemon spawned itself is backed by a pty here: {procs}"
    );
    assert!(
        context.get("remote").is_none(),
        "and it is not the near end of anything: {procs}"
    );
}
