use crate::address::Context;
use crate::backend::mock::MockBackend;
use crate::commands::Outcome;
use crate::commands::test_support::human;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;
use lyrel_core::core::agent_hooks::HooksState;
use lyrel_core::daemon::control::CONTROL_VERSION;
use lyrel_core::daemon::control::ReplyOk;
use lyrel_core::daemon::protocol::PROTOCOL_VERSION;
use serde_json::json;

fn doctor_backend() -> MockBackend {
    use lyrel_core::daemon::control::ServerStatus;

    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Status(ServerStatus {
        pid: 4242,
        uptime_secs: 61,
        panes: 3,
        control_version: CONTROL_VERSION,
        protocol_version: PROTOCOL_VERSION,
        build: "26.7.5".into(),
        socket: "127.0.0.1:5555".into(),
    }));
    backend.replies.push_back(ReplyOk::Routes(Vec::new()));
    backend
}

#[test]
fn doctor_reports_the_injected_context_and_the_server_half() {
    let out = human(run_cli(
        &["lyrel", "doctor"],
        &Context::default(),
        &mut doctor_backend(),
    ));
    assert!(out.contains("LYREL_CONFIG_DIR"), "{out}");
    assert!(out.contains("missing"), "{out}");
    assert!(out.contains("dialect"), "{out}");
    assert!(
        out.contains(&format!("control v{CONTROL_VERSION}")),
        "{out}"
    );
    assert!(out.contains("pid 4242"), "{out}");
    assert!(out.contains("0 known"), "{out}");

    let ctx = Context {
        pane: Some("7".into()),
        ws: None,
        config_dir: Some("/cfg/lyrel".into()),
    };
    let out = human(run_cli(&["lyrel", "doctor"], &ctx, &mut doctor_backend()));
    assert!(out.contains("set (/cfg/lyrel)"), "{out}");
}

/// Missing hooks are the reason a perfectly healthy-looking agent never
/// reports and `lyrel wait` sits there until it times out. `doctor` is the
/// verb people run when something is not working, so it is where that has
/// to be visible — and it long claimed to check hooks without doing so.
#[test]
fn doctor_reports_where_the_agent_status_hooks_stand() {
    use lyrel_core::core::agent_hooks::HookAgent;

    let mut backend = doctor_backend();
    // The real backend answers for every agent it knows how to install
    // hooks for, so the mock does too — the interesting part is that the
    // three states are told apart, not that a lookup can come back empty.
    backend.agent_hooks_states = HookAgent::ALL
        .into_iter()
        .map(|agent| match agent {
            HookAgent::OhMyPi => (agent, HooksState::Installed),
            HookAgent::Pi => (agent, HooksState::Outdated),
            other => (other, HooksState::NotInstalled),
        })
        .collect();
    let out = run_cli(&["lyrel", "doctor"], &Context::default(), &mut backend);
    let Outcome::Report(r) = out else {
        panic!("doctor reports");
    };
    assert!(r.human.contains("agent hooks"), "{}", r.human);
    assert!(
        r.human.contains("OUTDATED"),
        "an outdated hook is the quiet failure worth shouting about: {}",
        r.human
    );
    assert!(
        r.human.contains("Settings → Integrations"),
        "say where the fix is: {}",
        r.human
    );
    assert_eq!(r.json["hooks"]["installed"], serde_json::json!(["omp"]));
    assert_eq!(r.json["hooks"]["outdated"], serde_json::json!(["pi"]));
    assert_eq!(
        r.json["hooks"]["not_installed"]
            .as_array()
            .expect("the rest are reported as a list, not omitted")
            .len(),
        HookAgent::ALL.len() - 2
    );

    // A backend that cannot read hook state at all — a `-m` run, where the
    // hooks live on the other machine — says so rather than reporting a
    // machine-wide gap that is not there.
    let out = human(run_cli(
        &["lyrel", "doctor"],
        &Context::default(),
        &mut doctor_backend(),
    ));
    assert!(out.contains("unknown"), "{out}");
}

/// An unreachable server is *the* finding doctor exists for, so the verb
/// exits non-zero over it — `lyrel doctor || alert` has to fire — while
/// still printing the full report (#592).
#[test]
fn doctor_exits_nonzero_when_the_server_is_unreachable() {
    let mut backend = mock();
    backend.unreachable = true;
    let out = run_cli(&["lyrel", "doctor"], &Context::default(), &mut backend);
    let Outcome::Exit(1, r) = out else {
        panic!("an unreachable server is an exit 1, not a plain report: {out:?}");
    };
    assert_eq!(r.json["server"]["reachable"], serde_json::json!(false));
    // The rest of the report still goes out — the context rows are the
    // other half of what doctor is for.
    assert!(r.human.contains("LYREL_CONFIG_DIR"), "{}", r.human);
    assert!(r.human.contains("unreachable"), "{}", r.human);
    // No Status/Routes round-trips happen once hello has failed.
    assert!(
        backend.control_calls.is_empty(),
        "{:?}",
        backend.control_calls
    );
}
