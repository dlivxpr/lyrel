use crate::address::Context;
use crate::commands::Outcome;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::human;
use crate::commands::test_support::json_of;
use crate::commands::test_support::mock;
use crate::commands::test_support::pane_info;
use crate::commands::test_support::run_cli;
use lyrel_core::core::machine::Axis;
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;
use serde_json::Value;

#[test]
fn pane_ls_all_surfaces_the_panes_no_workspace_holds() {
    let mut backend = mock();
    // %1 and %3 are in the tree (see two_workspace_machine); %77 is what an
    // interrupted `lyrel run` leaves behind.
    backend.registry = vec![
        pane_info(1, None),
        pane_info(3, None),
        pane_info(77, Some("lyrel-cli")),
    ];

    let out = run_cli(
        &["lyrel", "pane", "ls", "--all"],
        &Context::default(),
        &mut backend,
    );
    let json = json_of(out);
    assert_eq!(json["orphans"], serde_json::json!(1), "{json}");

    let orphan = json["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pane"] == serde_json::json!(77))
        .expect("the orphan is listed");
    assert_eq!(orphan["orphan"], serde_json::json!(true));
    assert!(
        orphan["workspace"].is_null(),
        "no workspace holds it: {orphan}"
    );
    assert_eq!(orphan["owner"], serde_json::json!("lyrel-cli"), "{orphan}");

    let filed = json["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pane"] == serde_json::json!(1))
        .expect("the filed pane is listed too");
    assert_eq!(filed["orphan"], serde_json::json!(false));
    assert!(!filed["workspace"].is_null(), "{filed}");
}

#[test]
fn pane_ls_without_all_cannot_see_an_orphan() {
    let mut backend = mock();
    backend.registry = vec![pane_info(77, Some("lyrel-cli"))];
    let json = json_of(run_cli(
        &["lyrel", "pane", "ls"],
        &Context::default(),
        &mut backend,
    ));
    let listed: Vec<&Value> = json["panes"].as_array().unwrap().iter().collect();
    assert!(
        listed.iter().all(|p| p["pane"] != serde_json::json!(77)),
        "the tree-walking listing cannot reach the registry — that is why --all exists"
    );
}

#[test]
fn closing_an_orphan_falls_back_to_hanging_the_pane_up() {
    let mut backend = mock();
    // The registry must know %77: a direct kill is fire-and-forget, so
    // close verifies existence against it first (#588).
    backend.registry = vec![pane_info(77, Some("lyrel-cli"))];
    run_cli(
        &["lyrel", "pane", "close", "%77"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.killed,
        vec![77],
        "a pane no workspace holds must still be stoppable"
    );
    assert_eq!(
        backend.control_calls,
        vec![ControlRequest::MachineGet],
        "PaneClose needs a workspace, so it must not be attempted for an orphan"
    );

    // A pane the tree does hold still goes through PaneClose.
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Panes(vec![1]));
    run_cli(
        &["lyrel", "pane", "close", "%1"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(backend.killed, vec![1], "the removed pane is hung up");
    assert!(
        backend
            .control_calls
            .iter()
            .any(|c| matches!(c, ControlRequest::PaneClose { pane: 1, .. })),
        "{:?}",
        backend.control_calls
    );
}

#[test]
fn closing_a_pane_that_never_existed_is_a_failure_not_a_ghost_success() {
    // %99 is in no workspace and in no registry — exactly the typo a
    // reaper script makes. Closing it used to print {"closed":[99]} and
    // exit 0, telling the script the leak it was chasing was gone (#588).
    let mut backend = mock();
    backend.registry = vec![pane_info(77, Some("lyrel-cli"))];
    let out = execute(
        cli(&["lyrel", "pane", "close", "%99"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a ghost close is an exit code, not an error");
    let Outcome::Exit(1, r) = out else {
        panic!("closing a pane that does not exist has to fail: {out:?}");
    };
    assert_eq!(r.json["closed"], serde_json::json!([]));
    assert!(
        r.json["failed"]
            .as_array()
            .expect("the failures are a list")
            .iter()
            .any(|f| f.as_str().is_some_and(|f| f.contains("%99"))),
        "{}",
        r.json
    );
    assert!(
        backend.killed.is_empty(),
        "no kill may be sent for a pane the registry does not hold"
    );
}

/// The listing is where an orphan is found, so it is where the way out of
/// being one has to be written down.
#[test]
fn pane_ls_all_points_at_the_way_back_as_well_as_the_way_out() {
    let mut backend = mock();
    backend.registry = vec![pane_info(37, None)];
    let out = human(run_cli(
        &["lyrel", "pane", "ls", "--all"],
        &Context::default(),
        &mut backend,
    ));
    assert!(out.contains("lyrel tab new --pane %<id>"), "{out}");
    assert!(out.contains("lyrel pane close --orphans"), "{out}");
}

#[test]
fn pane_split_builds_the_split_and_spawns_the_new_shell() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].id;
    let out = run_cli(
        &["lyrel", "pane", "split", "%2", "--v", "--ratio", "0.3"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.control_calls,
        vec![
            ControlRequest::MachineGet,
            ControlRequest::PaneSplit {
                workspace: api,
                pane: 2,
                axis: Axis::Vertical,
                ratio: 0.3,
                new: PaneSeed {
                    pane: 6,
                    cwd: Some("C:\\proj".into()),
                    ssh_spec: None,
                    agent: None,
                    shell: None,
                },
                first: false,
            },
        ],
        "the new pane inherits the split pane's cwd"
    );
    assert_eq!(backend.spawned, vec![(api, Some("C:\\proj".to_string()))]);
    assert_eq!(
        human(out),
        "%6",
        "the new pane address is the printed result"
    );
}

#[test]
fn split_without_an_address_uses_the_pane_from_the_environment() {
    let mut backend = mock();
    let ctx = Context {
        pane: Some("5".into()),
        ..Context::default()
    };
    run_cli(&["lyrel", "split", "--h"], &ctx, &mut backend);
    let web = backend.machine.workspaces[1].id;
    match &backend.control_calls[1] {
        ControlRequest::PaneSplit {
            workspace,
            pane,
            axis,
            ..
        } => {
            assert_eq!(*workspace, web);
            assert_eq!(*pane, 5);
            assert_eq!(*axis, Axis::Horizontal);
        }
        other => panic!("expected PaneSplit, got {other:?}"),
    }
}

#[test]
fn pane_close_traces_the_pane_to_its_workspace() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Panes(vec![5]));
    run_cli(
        &["lyrel", "pane", "close", "%5"],
        &Context::default(),
        &mut backend,
    );
    let web = backend.machine.workspaces[1].id;
    assert_eq!(
        backend.control_calls,
        vec![
            ControlRequest::MachineGet,
            ControlRequest::PaneClose {
                workspace: web,
                pane: 5,
            },
        ]
    );
    assert_eq!(
        backend.killed,
        vec![5],
        "a pane removed from its workspace must also be hung up"
    );
}

/// The CLI is what creates orphans, so it should be able to clear them.
/// `--orphans` closes exactly the panes the registry holds and the tab
/// trees do not — panes that *are* held must survive it untouched.
#[test]
fn pane_close_orphans_reaps_only_what_no_workspace_holds() {
    let mut backend = mock();
    // %1 and %3 live in the tree (see two_workspace_machine); %77 and %78
    // are what interrupted `run`s left behind.
    backend.registry = vec![
        pane_info(1, None),
        pane_info(3, None),
        pane_info(77, Some("lyrel-cli")),
        pane_info(78, Some("lyrel-cli")),
    ];

    let json = json_of(run_cli(
        &["lyrel", "pane", "close", "--orphans"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["closed"], serde_json::json!([77, 78]));
    assert_eq!(backend.killed, vec![77, 78]);
    assert!(
        !backend
            .control_calls
            .iter()
            .any(|c| matches!(c, ControlRequest::PaneClose { .. })),
        "orphans have no workspace to route a PaneClose through"
    );

    // Nothing to reap is a success with an empty list, not an error: a
    // cleanup step that fails when the machine is already clean is one a
    // script has to guard, and every script would then guard it the same way.
    let mut backend = mock();
    backend.registry = vec![pane_info(1, None), pane_info(3, None)];
    let json = json_of(run_cli(
        &["lyrel", "pane", "close", "--orphans"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["closed"], serde_json::json!([]));
    assert!(backend.killed.is_empty());
}

/// A batch keeps going after a failure. Stopping at the first one would
/// leave the rest of the leak exactly where it was — while still reporting
/// the failure, because a half-done cleanup that claims success is worse.
///
/// Reported as an exit code carrying a report, not as an error: the caller
/// was cleaning up, and the useful answer is which panes are still theirs
/// to deal with. An anyhow error would leave `--json` with prose.
#[test]
fn pane_close_reports_failures_without_abandoning_the_batch() {
    let mut backend = mock();
    backend.registry = vec![
        pane_info(77, None),
        pane_info(78, None),
        pane_info(79, None),
    ];
    backend.kill_failures = vec![78];

    let out = execute(
        cli(&["lyrel", "pane", "close", "--orphans"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a partial cleanup is an exit code, not an error");
    let Outcome::Exit(1, r) = out else {
        panic!("a pane that could not be closed has to be reported");
    };
    assert_eq!(
        r.json["closed"],
        serde_json::json!([77, 79]),
        "the survivors of the batch are what a retry needs: {}",
        r.json
    );
    assert!(
        r.json["failed"]
            .as_array()
            .expect("the failures are a list")
            .iter()
            .any(|f| f.as_str().is_some_and(|f| f.contains("%78"))),
        "{}",
        r.json
    );
    assert_eq!(
        backend.killed,
        vec![77, 78, 79],
        "the panes after the failure still had to be attempted"
    );
}
