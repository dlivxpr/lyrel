use crate::address::Context;
use crate::backend::mock::MockBackend;
use crate::commands::Outcome;
use crate::commands::test_support::human;
use crate::commands::test_support::json_of;
use crate::commands::test_support::mock;
use crate::commands::test_support::pane_info;
use crate::commands::test_support::run_cli;
use crate::commands::test_support::segment;
use crate::resolve;
use lyrel_core::core::machine::Machine;
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::core::machine::Tab;
use lyrel_core::core::machine::Workspace;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;
use serde_json::json;

#[test]
fn ls_and_ws_ls_are_the_same_request() {
    let ctx = Context::default();
    let mut a = mock();
    run_cli(&["lyrel", "ls"], &ctx, &mut a);
    let mut b = mock();
    run_cli(&["lyrel", "ws", "ls"], &ctx, &mut b);
    assert_eq!(a.control_calls, vec![ControlRequest::MachineGet]);
    assert_eq!(a.control_calls, b.control_calls, "the alias must not drift");
}

#[test]
fn sidecar_projection_is_not_a_workspace_but_its_native_pane_remains_addressable() {
    let ctx = Context::default();
    let mut backend = mock();
    let (projection, native) = crate::testbed::mixed_sidecar_projection();
    let projection_id = projection.id;
    backend.registry = vec![pane_info(77, Some(&projection_id.to_string()))];
    backend.machine.workspaces.insert(0, projection);
    backend.machine.panes.push(native);

    for args in [vec!["lyrel", "ls"], vec!["lyrel", "ws", "ls"]] {
        let Outcome::Report(report) = run_cli(&args, &ctx, &mut backend) else {
            panic!("a workspace listing is a report");
        };
        let listed = report.json["workspaces"].as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "api");
        assert_eq!(listed[1]["name"], "web");
        assert!(!report.human.contains("internal-projection"));
        assert!(!report.human.contains(&resolve::short_id(&projection_id)));
    }

    let Outcome::Report(report) = run_cli(&["lyrel", "pane", "ls"], &ctx, &mut backend) else {
        panic!("a pane listing is a report");
    };
    assert!(
        report.json["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pane| pane["pane"] == 77)
    );
    assert!(report.human.contains("%77"));
    assert!(!report.human.contains("@0"));

    let out = run_cli(&["lyrel", "pane", "ls", "--all"], &ctx, &mut backend);
    let listed = json_of(out);
    assert_eq!(listed["panes"][0]["pane"], 77);
    assert_eq!(listed["panes"][0]["workspace"], projection_id.to_string());
    assert_eq!(listed["panes"][0]["orphan"], false);
    assert_eq!(listed["orphans"], 0);

    run_cli(
        &["lyrel", "send", "%77", "native input"],
        &ctx,
        &mut backend,
    );
    assert_eq!(backend.sent, vec![(77, b"native input".to_vec())]);
    backend.capture_segments = vec![segment(b"native output")];
    let captured = run_cli(&["lyrel", "capture", "%77"], &ctx, &mut backend);
    assert_eq!(json_of(captured)["text"], "native output");
    assert_eq!(backend.captured, vec![(77, false)]);

    let out = run_cli(&["lyrel", "pane", "close", "--orphans"], &ctx, &mut backend);
    assert_eq!(json_of(out)["closed"], json!([]));
    assert!(
        backend.killed.is_empty(),
        "a projection pane is not an orphan"
    );
    backend.replies.push_back(ReplyOk::Panes(vec![77]));
    let out = run_cli(&["lyrel", "pane", "close", "%77"], &ctx, &mut backend);
    assert_eq!(json_of(out)["closed"], json!([77]));
    assert!(backend.control_calls.contains(&ControlRequest::PaneClose {
        workspace: projection_id,
        pane: 77,
    }));
    assert_eq!(backend.killed, vec![77]);
}

#[test]
fn sidecar_projection_only_machine_has_the_normal_empty_workspace_listing() {
    let (projection, native) = crate::testbed::mixed_sidecar_projection();
    let mut backend = MockBackend::with_machine(Machine {
        workspaces: vec![projection],
        panes: vec![native],
    });
    let Outcome::Report(report) =
        run_cli(&["lyrel", "ws", "ls"], &Context::default(), &mut backend)
    else {
        panic!("a workspace listing is a report");
    };
    assert_eq!(report.json["workspaces"], json!([]));
    assert_eq!(
        report.human,
        "no workspaces — `lyrel new <path>` starts one\n"
    );
}

#[test]
fn ws_tree_asks_for_the_resolved_workspace() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].clone();
    backend
        .replies
        .push_back(ReplyOk::WorkspaceTree(Box::new(api.clone())));
    run_cli(
        &["lyrel", "ws", "tree", "api"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.control_calls,
        vec![
            ControlRequest::MachineGet,
            ControlRequest::WorkspaceTree { workspace: api.id },
        ]
    );
}

#[test]
fn ws_new_carries_the_name() {
    let mut backend = mock();
    let created = Workspace::default();
    backend
        .replies
        .push_back(ReplyOk::WorkspaceTree(Box::new(created.clone())));
    let out = run_cli(
        &["lyrel", "ws", "new", "dev"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.control_calls,
        vec![ControlRequest::WorkspaceCreate {
            name: Some("dev".into()),
            workspace: None,
        }]
    );
    assert_eq!(
        human(out),
        created.id.to_string(),
        "the id is the printed result"
    );
}

#[test]
fn new_spawns_first_and_seeds_the_tab_with_the_daemons_pane_id() {
    let mut backend = mock();
    let created = Workspace::default();
    backend
        .replies
        .push_back(ReplyOk::WorkspaceTree(Box::new(created.clone())));
    backend
        .replies
        .push_back(ReplyOk::TabTree(Box::new(Tab::leaf(6))));
    let out = run_cli(
        &["lyrel", "new", "C:\\newproj"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.control_calls,
        vec![
            ControlRequest::WorkspaceCreate {
                name: None,
                workspace: None,
            },
            ControlRequest::TabCreate {
                workspace: created.id,
                at: None,
                pane: PaneSeed {
                    pane: 6,
                    cwd: Some("C:\\newproj".into()),
                    ssh_spec: None,
                    agent: None,
                    shell: None,
                },
                tab: None,
            },
        ],
        "the daemon-assigned pane id (6) lands in the tree op, so the spawn came first"
    );
    assert_eq!(
        backend.spawned,
        vec![(created.id, Some("C:\\newproj".to_string()))],
        "the tree op alone leaves a dead pane — the shell must be spawned"
    );
    assert_eq!(human(out), created.id.to_string());
}

#[test]
fn ws_rename_rm_attach_detach_build_their_requests() {
    let ctx = Context::default();
    let mut backend = mock();
    let api = backend.machine.workspaces[0].id;
    let web = backend.machine.workspaces[1].id;

    run_cli(
        &["lyrel", "ws", "rename", "api", "core"],
        &ctx,
        &mut backend,
    );
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::WorkspaceRename {
            workspace: api,
            name: Some("core".into()),
        }
    );

    backend.control_calls.clear();
    backend.replies.push_back(ReplyOk::Panes(vec![3, 4]));
    run_cli(&["lyrel", "ws", "rm", "web"], &ctx, &mut backend);
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::WorkspaceRemove { workspace: web }
    );
    assert_eq!(
        backend.killed,
        vec![3, 4],
        "removing a workspace must hang up the panes it held"
    );

    backend.control_calls.clear();
    backend.replies.push_back(ReplyOk::Attached {
        took_over_from: Some("laptop".into()),
    });
    let out = run_cli(&["lyrel", "ws", "attach", "api"], &ctx, &mut backend);
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::WorkspaceAttach {
            id: api.to_string(),
        }
    );
    assert_eq!(human(out), "took over from laptop");

    backend.control_calls.clear();
    run_cli(&["lyrel", "ws", "detach", "api"], &ctx, &mut backend);
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::WorkspaceDetach {
            id: api.to_string(),
        }
    );
}
