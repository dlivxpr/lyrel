use anyhow::{Result, bail};
use lyrel_core::core::agent_hooks::{HookAgent, HooksState};
use lyrel_core::core::machine::Machine;
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::{Value, json};

use super::target::fetch_machine;
use super::{Outcome, report};
use crate::backend::Backend;
use crate::output;

pub(super) fn agents(backend: &mut dyn Backend) -> Result<Outcome> {
    match backend.control(ControlRequest::AgentStates)? {
        ReplyOk::AgentStates(states) => {
            // AgentStates only contains panes that have already emitted a hook
            // event. The machine snapshot independently records the daemon's
            // foreground-process detection, including a supported agent that
            // has not been able to report yet.
            let diagnostics = fetch_machine(backend)
                .map(|machine| agent_hook_diagnostics(&states, &machine, backend))
                .unwrap_or_default();
            let mut json = json!({ "agents": serde_json::to_value(&states)? });
            if !diagnostics.is_empty() {
                json["diagnostics"] =
                    Value::Array(diagnostics.iter().map(AgentHookDiagnostic::json).collect());
            }
            report(agents_human(&states, &diagnostics), json)
        }
        other => bail!("the server answered AgentStates with {other:?}"),
    }
}

/// The two hook states worth reporting. Building one of these is the only way
/// to reach a diagnostic, so "installed hooks are not a diagnostic" is a shape
/// the type cannot hold rather than a branch that has to stay unreachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HookGap {
    Missing,
    Outdated,
}

impl HookGap {
    fn of(state: HooksState) -> Option<HookGap> {
        match state {
            HooksState::NotInstalled => Some(HookGap::Missing),
            HooksState::Outdated => Some(HookGap::Outdated),
            HooksState::Installed => None,
        }
    }

    /// The verb of the Settings button that closes the gap.
    fn action(self) -> &'static str {
        match self {
            HookGap::Missing => "install",
            HookGap::Outdated => "update",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            HookGap::Missing => "not_installed",
            HookGap::Outdated => "outdated",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            HookGap::Missing => "not installed",
            HookGap::Outdated => "outdated",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentHookDiagnostic {
    agent: HookAgent,
    gap: HookGap,
}

impl AgentHookDiagnostic {
    fn json(&self) -> Value {
        json!({
            "kind": "agent_status_hooks_unavailable",
            "agent": self.agent.slug(),
            "hooks_state": self.gap.slug(),
            "action": self.gap.action(),
        })
    }
}

fn agent_hook_diagnostics(
    states: &[lyrel_core::daemon::control::PaneAgentState],
    machine: &Machine,
    backend: &mut dyn Backend,
) -> Vec<AgentHookDiagnostic> {
    HookAgent::ALL
        .into_iter()
        .filter(|hook_agent| {
            machine.panes.iter().any(|pane| {
                pane.live
                    && !states.iter().any(|state| state.pane_id == pane.id)
                    && pane.agent.as_ref().is_some_and(|facts| {
                        HookAgent::of_detected(facts.agent) == Some(*hook_agent)
                    })
            })
        })
        .filter_map(|agent| {
            let gap = HookGap::of(backend.agent_hooks_state(agent)?)?;
            Some(AgentHookDiagnostic { agent, gap })
        })
        .collect()
}

fn agents_human(
    states: &[lyrel_core::daemon::control::PaneAgentState],
    diagnostics: &[AgentHookDiagnostic],
) -> String {
    if diagnostics.is_empty() {
        return output::agents_table(states);
    }
    let mut human = if states.is_empty() {
        "no agents reporting status\n".to_string()
    } else {
        output::agents_table(states)
    };
    for diagnostic in diagnostics {
        let state = diagnostic.gap.describe();
        human.push_str(&format!(
            "{} is running, but its lyrel agent-status hooks are {state}. Open Settings → Integrations to {} the hooks, then start a new {} session.\n",
            diagnostic.agent.display_name(),
            diagnostic.gap.action(),
            diagnostic.agent.display_name(),
        ));
    }
    human
}

#[cfg(test)]
mod tests;
