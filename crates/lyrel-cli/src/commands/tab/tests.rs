use crate::address::Context;
use crate::backend::mock::MockBackend;
use crate::commands::Outcome;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::human;
use crate::commands::test_support::mock;
use crate::commands::test_support::pane_info;
use crate::commands::test_support::run_cli;
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::core::machine::Tab;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;
use serde_json::Value;
use serde_json::json;

#[test]
fn tab_verbs_resolve_machine_wide_ordinals_to_real_ids() {
    let ctx = Context::default();
    let mut backend = mock();
    let web = backend.machine.workspaces[1].clone();

    backend.replies.push_back(ReplyOk::Panes(Vec::new()));
    run_cli(&["lyrel", "tab", "close", "@3"], &ctx, &mut backend);
    assert_eq!(
        backend.control_calls,
        vec![
            ControlRequest::MachineGet,
            ControlRequest::TabClose {
                workspace: web.id,
                tab: web.tabs[0].id,
            },
        ]
    );

    backend.control_calls.clear();
    let api = backend.machine.workspaces[0].clone();
    run_cli(
        &["lyrel", "tab", "rename", "@1", "build2"],
        &ctx,
        &mut backend,
    );
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::TabRename {
            workspace: api.id,
            tab: api.tabs[0].id,
            name: Some("build2".into()),
        }
    );

    backend.control_calls.clear();
    run_cli(&["lyrel", "tab", "move", "@2", "0"], &ctx, &mut backend);
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::TabMove {
            workspace: api.id,
            tab: api.tabs[1].id,
            to: 0,
        }
    );
}

/// File tab @2 under a pinned folder group, the way the GUI would.
fn pin_second_tab(backend: &mut MockBackend, name: Option<&str>) -> String {
    use lyrel_core::core::group_key::PinnedGroup;
    let mut group = PinnedGroup::folder(std::path::Path::new("C:\\proj\\sub"));
    group.name = name.map(str::to_string);
    let id = group.id;
    let ws = &mut backend.machine.workspaces[0];
    ws.groups.pinned.push(group);
    ws.tabs[1].group = Some(id);
    id.to_string()
}

#[test]
fn tab_ls_names_an_unnamed_tab_and_shows_the_leaf_of_its_group() {
    let mut backend = mock();
    pin_second_tab(&mut backend, None);

    let out = run_cli(
        &["lyrel", "tab", "ls", "api"],
        &Context::default(),
        &mut backend,
    );

    // @1 was named; @2 was not, so it borrows the leaf of its cwd. The
    // GROUP column is what the header reads — the folder's last segment
    // for a group nobody renamed, not the whole path.
    assert_eq!(
        human(out),
        "TAB  NAME   GROUP  PANES\n@1   build  -      1\n@2   proj   sub    2\n"
    );
}

#[test]
fn tab_ls_shows_a_renamed_group_by_its_name() {
    let mut backend = mock();
    pin_second_tab(&mut backend, Some("work"));

    let out = run_cli(
        &["lyrel", "tab", "ls", "api"],
        &Context::default(),
        &mut backend,
    );

    assert_eq!(
        human(out),
        "TAB  NAME   GROUP  PANES\n@1   build  -      1\n@2   proj   work   2\n"
    );
}

/// A tab naming a group the workspace no longer has is an auto tab, as
/// the sidebar draws it.
#[test]
fn tab_ls_shows_no_group_for_a_tab_naming_a_deleted_one() {
    let mut backend = mock();
    pin_second_tab(&mut backend, None);
    backend.machine.workspaces[0].groups.pinned.clear();

    let out = run_cli(
        &["lyrel", "tab", "ls", "api"],
        &Context::default(),
        &mut backend,
    );

    assert_eq!(
        human(out),
        "TAB  NAME   GROUP  PANES\n@1   build  -      1\n@2   proj   -      2\n"
    );
}

#[test]
fn tab_ls_json_keeps_the_literal_name_beside_the_label() {
    let mut backend = mock();
    let id = pin_second_tab(&mut backend, None);

    let out = run_cli(
        &["lyrel", "tab", "ls", "api"],
        &Context::default(),
        &mut backend,
    );

    let Outcome::Report(report) = out else {
        panic!("tab ls must report");
    };
    let tabs = report.json["tabs"].as_array().expect("tabs").clone();
    assert_eq!(tabs[1]["name"], Value::Null, "nobody named this tab");
    assert_eq!(tabs[1]["label"], "proj", "the table's stand-in travels too");
    assert_eq!(
        tabs[1]["group"],
        json!({ "id": id, "name": null, "folder": "C:\\proj\\sub" }),
        "the JSON keeps the whole folder the table abbreviates"
    );
    assert_eq!(tabs[0]["group"], Value::Null, "an auto tab names no group");
}

#[test]
fn tab_close_hangs_up_every_pane_the_server_removed() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Panes(vec![2, 3]));

    run_cli(
        &["lyrel", "tab", "close", "@2"],
        &Context::default(),
        &mut backend,
    );

    assert_eq!(
        backend.killed,
        vec![2, 3],
        "every pane removed with the tab must be hung up"
    );
}

#[test]
fn tab_close_attempts_every_hangup_before_reporting_failures() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Panes(vec![2, 3]));
    backend.kill_failures.push(2);

    let error = execute(
        cli(&["lyrel", "tab", "close", "@2"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("a failed pane hangup must fail tab close");

    assert_eq!(
        backend.killed,
        vec![2, 3],
        "a failed hangup must not skip the remaining panes"
    );
    assert!(error.to_string().contains("%2"), "{error:#}");
}

#[test]
fn tab_close_rejects_an_unexpected_server_reply() {
    let mut backend = mock();
    let error = execute(
        cli(&["lyrel", "tab", "close", "@2"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("TabClose must return the panes it removed");

    assert!(
        error
            .to_string()
            .contains("the server answered TabClose with Unit"),
        "{error:#}"
    );
}

#[test]
fn tab_new_uses_the_workspace_from_the_environment() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].clone();
    backend
        .replies
        .push_back(ReplyOk::TabTree(Box::new(Tab::leaf(6))));
    let ctx = Context {
        ws: Some(api.id.to_string()),
        ..Context::default()
    };
    run_cli(
        &["lyrel", "tab", "new", "--cwd", "C:\\elsewhere"],
        &ctx,
        &mut backend,
    );
    assert_eq!(
        backend.control_calls[1],
        ControlRequest::TabCreate {
            workspace: api.id,
            at: None,
            pane: PaneSeed {
                pane: 6,
                cwd: Some("C:\\elsewhere".into()),
                ssh_spec: None,
                agent: None,
                shell: None,
            },
            tab: None,
        }
    );
    assert_eq!(
        backend.spawned,
        vec![(api.id, Some("C:\\elsewhere".to_string()))]
    );
}

/// The recovery verb from #716. The pane is running and no tab holds it;
/// the tab is built around it and no new shell is started.
#[test]
fn tab_new_with_a_pane_re_homes_an_orphan_instead_of_spawning() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].clone();
    let mut orphan = pane_info(37, Some(&api.id.to_string()));
    orphan.cwd = Some("C:\\work".into());
    backend.registry = vec![orphan];
    backend
        .replies
        .push_back(ReplyOk::TabTree(Box::new(Tab::leaf(37))));

    let out = run_cli(
        &["lyrel", "tab", "new", "--pane", "%37"],
        &Context::default(),
        &mut backend,
    );

    assert_eq!(
        backend.control_calls[1],
        ControlRequest::TabCreate {
            workspace: api.id,
            at: None,
            pane: PaneSeed {
                pane: 37,
                // Rebuilt from the registry: the tree dropped this pane's
                // record when the tab holding it closed.
                cwd: Some("C:\\work".into()),
                ssh_spec: None,
                agent: None,
                shell: None,
            },
            tab: None,
        },
        "with no workspace named, the pane goes back to the one it was spawned for"
    );
    assert!(
        backend.spawned.is_empty(),
        "re-homing must not start a second shell — the point is the one still running"
    );
    assert_eq!(human(out), "%37");
}

#[test]
fn tab_new_with_a_pane_takes_an_explicit_workspace_and_cwd() {
    let mut backend = mock();
    let web = backend.machine.workspaces[1].id;
    backend.registry = vec![pane_info(37, None)];
    backend
        .replies
        .push_back(ReplyOk::TabTree(Box::new(Tab::leaf(37))));

    run_cli(
        &[
            "lyrel", "tab", "new", "web", "--pane", "%37", "--cwd", "C:\\else",
        ],
        &Context::default(),
        &mut backend,
    );

    assert_eq!(
        backend.control_calls[1],
        ControlRequest::TabCreate {
            workspace: web,
            at: None,
            pane: PaneSeed {
                pane: 37,
                cwd: Some("C:\\else".into()),
                ssh_spec: None,
                agent: None,
                shell: None,
            },
            tab: None,
        },
        "a pane with no owner still re-homes wherever it is told to"
    );
}

#[test]
fn tab_new_refuses_a_pane_that_is_not_running() {
    let mut backend = mock();
    let error = execute(
        cli(&["lyrel", "tab", "new", "api", "--pane", "%99"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("a pane the server does not hold cannot be re-homed");
    assert!(
        error.to_string().contains("no pane %99 is running"),
        "{error:#}"
    );
    assert!(
        !backend
            .control_calls
            .iter()
            .any(|call| matches!(call, ControlRequest::TabCreate { .. })),
        "and nothing is written to the tree"
    );
}

/// A pane a tab already holds is not an orphan, and putting it in a second
/// tab would leave the tree with one pane in two places.
#[test]
fn tab_new_refuses_a_pane_a_tab_already_holds() {
    let mut backend = mock();
    backend.registry = vec![pane_info(2, None)];
    let error = execute(
        cli(&["lyrel", "tab", "new", "api", "--pane", "%2"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("%2 is in a tab of api");
    assert!(error.to_string().contains("already in a tab"), "{error:#}");
}

#[test]
fn tab_ls_and_pane_ls_say_which_tabs_are_asleep() {
    let mut backend = mock();
    backend.machine.workspaces[0].tabs[1].hibernated = true;

    let Outcome::Report(tabs) = run_cli(
        &["lyrel", "tab", "ls", "api"],
        &Context::default(),
        &mut backend,
    ) else {
        panic!("tab ls must report");
    };
    let tabs = tabs.json["tabs"].as_array().expect("tabs").clone();
    assert_eq!(tabs[0]["hibernated"], false);
    assert_eq!(tabs[1]["hibernated"], true);

    let Outcome::Report(panes) = run_cli(
        &["lyrel", "pane", "ls", "api"],
        &Context::default(),
        &mut backend,
    ) else {
        panic!("pane ls must report");
    };
    let panes = panes.json["panes"].as_array().expect("panes").clone();
    let asleep: Vec<u64> = panes
        .iter()
        .filter(|p| p["hibernated"] == true)
        .map(|p| p["pane"].as_u64().unwrap())
        .collect();
    assert_eq!(asleep, vec![2, 3]);
}
