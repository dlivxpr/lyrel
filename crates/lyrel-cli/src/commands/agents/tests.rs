use crate::address::Context;
use crate::backend::mock::MockBackend;
use crate::commands::Outcome;
use crate::commands::test_support::agent_state;
use crate::commands::test_support::human;
use crate::commands::test_support::json_of;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;
use lyrel_core::core::agent_hooks::HookAgent;
use lyrel_core::core::agent_hooks::HooksState;
use lyrel_core::core::cli_agent::CLIAgent;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;
use serde_json::json;

fn detect_agent(backend: &mut MockBackend, pane_id: u64, agent: CLIAgent) {
    use lyrel_core::core::machine::AgentFacts;

    let pane = backend
        .machine
        .panes
        .iter_mut()
        .find(|pane| pane.id == pane_id)
        .expect("test pane exists");
    pane.live = true;
    pane.agent = Some(AgentFacts {
        agent,
        session_id: None,
        launch_argv: None,
        session_scope: None,
        status: None,
    });
}

#[test]
fn agents_distinguishes_no_agent_from_a_healthy_agent() {
    use lyrel_core::core::cli_agent::AgentStatus;

    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    let out = run_cli(&["lyrel", "agents"], &Context::default(), &mut backend);
    assert_eq!(
        backend.control_calls,
        vec![ControlRequest::AgentStates, ControlRequest::MachineGet]
    );
    assert_eq!(human(out), "no agents running\n");

    let mut backend = mock();
    detect_agent(&mut backend, 1, CLIAgent::Pi);
    backend.agent_hooks_states = vec![(HookAgent::Pi, HooksState::NotInstalled)];
    let mut reporting = agent_state(1, AgentStatus::Working);
    reporting.agent = Some(CLIAgent::Pi);
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![reporting]));
    let out = run_cli(&["lyrel", "agents"], &Context::default(), &mut backend);
    let rendered = human(out);
    assert!(rendered.contains("pi"), "{rendered}");
    assert!(
        !rendered.contains("hooks"),
        "an agent already reporting status is healthy: {rendered}"
    );

    let mut backend = mock();
    detect_agent(&mut backend, 1, CLIAgent::Pi);
    backend.agent_hooks_states = vec![(HookAgent::Pi, HooksState::Installed)];
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    let out = run_cli(&["lyrel", "agents"], &Context::default(), &mut backend);
    assert_eq!(
        human(out),
        "no agents running\n",
        "installed hooks are not diagnosed during the gap before the first event"
    );
}

#[test]
fn agents_reports_missing_hooks_once_per_agent_without_failing() {
    let mut backend = mock();
    detect_agent(&mut backend, 1, CLIAgent::Pi);
    detect_agent(&mut backend, 2, CLIAgent::Pi);
    backend.agent_hooks_states = vec![(HookAgent::Pi, HooksState::NotInstalled)];
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));

    let out = run_cli(&["lyrel", "agents"], &Context::default(), &mut backend);
    let Outcome::Report(report) = out else {
        panic!("a successful diagnosis must not be a command failure");
    };
    assert_eq!(report.human.matches("Pi is running").count(), 1);
    assert!(report.human.contains("hooks are not installed"));
    assert!(report.human.contains("install the hooks"));
    assert_eq!(
        report.json,
        json!({
            "agents": [],
            "diagnostics": [{
                "kind": "agent_status_hooks_unavailable",
                "agent": "pi",
                "hooks_state": "not_installed",
                "action": "install",
            }],
        })
    );
}

#[test]
fn agents_reports_outdated_hooks_with_the_update_action() {
    let mut backend = mock();
    detect_agent(&mut backend, 3, CLIAgent::OhMyPi);
    backend.agent_hooks_states = vec![(HookAgent::OhMyPi, HooksState::Outdated)];
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));

    let report = match run_cli(&["lyrel", "agents"], &Context::default(), &mut backend) {
        Outcome::Report(report) => report,
        Outcome::Exit(code, _) => panic!("diagnosis unexpectedly exited {code}"),
    };
    assert!(report.human.contains("Oh My Pi is running"));
    assert!(report.human.contains("hooks are outdated"));
    assert!(report.human.contains("update the hooks"));
    assert_eq!(report.json["diagnostics"][0]["agent"], "omp");
    assert_eq!(report.json["diagnostics"][0]["hooks_state"], "outdated");
    assert_eq!(report.json["diagnostics"][0]["action"], "update");
}

#[test]
fn agents_json_keeps_the_existing_agents_shape_when_there_is_no_diagnostic() {
    use lyrel_core::core::cli_agent::AgentStatus;

    let mut backend = mock();
    let mut reporting = agent_state(1, AgentStatus::Waiting);
    reporting.agent = Some(CLIAgent::Pi);
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![reporting.clone()]));
    let json = json_of(run_cli(
        &["lyrel", "--json", "agents"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json, json!({ "agents": [reporting] }));
    assert!(json.get("diagnostics").is_none());
}
