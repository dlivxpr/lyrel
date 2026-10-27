use clap::Parser;
use crate::address::Context;
use crate::backend::mock::MockBackend;
use crate::cli::Cli;
use crate::commands::Outcome;
use crate::commands::execute;
use crate::testbed::two_workspace_machine;
use serde_json::Value;
use serde_json::json;

pub(super) fn cli(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).expect("test invocations use the documented grammar")
}

pub(super) fn mock() -> MockBackend {
    MockBackend::with_machine(two_workspace_machine())
}

/// A replayed snapshot at a 20-column pane, which is narrow enough that the
/// wrapping tests can wrap without pages of fixture.
pub(super) fn segment(bytes: &[u8]) -> crate::backend::CaptureSegment {
    crate::backend::CaptureSegment {
        size: lyrel_core::daemon::protocol::WinSize {
            cols: 20,
            rows: 10,
            cell_w: 8,
            cell_h: 16,
        },
        bytes: bytes.to_vec(),
    }
}

pub(super) fn run_cli(args: &[&str], ctx: &Context, backend: &mut MockBackend) -> Outcome {
    execute(cli(args), ctx, backend).expect("this command should succeed against the mock")
}

pub(super) fn human(outcome: Outcome) -> String {
    match outcome {
        Outcome::Report(r) => r.human,
        Outcome::Exit(code, _) => panic!("expected a report, got exit {code}"),
    }
}

pub(super) fn json_of(outcome: Outcome) -> Value {
    match outcome {
        Outcome::Report(r) => r.json,
        Outcome::Exit(code, _) => panic!("expected a report, got exit {code}"),
    }
}

pub(super) fn pane_info(pane_id: u64, owner: Option<&str>) -> lyrel_core::daemon::protocol::PaneInfo {
    lyrel_core::daemon::protocol::PaneInfo {
        pane_id,
        cwd: None,
        title: "sh".into(),
        osc_title: None,
        alive: true,
        owner: owner.map(str::to_string),
    }
}

pub(super) fn agent_state(
    pane_id: u64,
    status: lyrel_core::core::cli_agent::AgentStatus,
) -> lyrel_core::daemon::control::PaneAgentState {
    agent_state_at(pane_id, status, 0)
}

pub(super) fn agent_state_at(
    pane_id: u64,
    status: lyrel_core::core::cli_agent::AgentStatus,
    activity: u64,
) -> lyrel_core::daemon::control::PaneAgentState {
    lyrel_core::daemon::control::PaneAgentState {
        pane_id,
        agent: None,
        state: lyrel_core::core::cli_agent::AgentSessionState {
            status,
            message: Some("needs permission".into()),
            session_id: Some("sess-9".into()),
            activity,
            ..Default::default()
        },
    }
}
