use anyhow::{Result, bail};
use lyrel_core::core::agent_hooks::{HookAgent, HooksState};
use lyrel_core::daemon::control::{CONTROL_VERSION, ControlRequest, ReplyOk};
use lyrel_core::daemon::protocol::PROTOCOL_VERSION;
use serde_json::{Value, json};

use super::{Outcome, Report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::output;

pub(super) fn doctor(ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let mark = |v: &Option<String>| match v {
        Some(value) => format!("set ({value})"),
        None => "missing".to_string(),
    };
    let mut rows = vec![
        vec![address::ENV_CONFIG_DIR.to_string(), mark(&ctx.config_dir)],
        vec![address::ENV_WS.to_string(), mark(&ctx.ws)],
        vec![address::ENV_PANE.to_string(), mark(&ctx.pane)],
    ];
    let mut server = json!({ "reachable": false });
    let mut hooks: Vec<(HookAgent, HooksState)> = Vec::new();
    match backend.hello() {
        Ok(hello) => {
            let dialect_ok = hello.control_version == CONTROL_VERSION
                && hello.protocol_version == PROTOCOL_VERSION;
            let dialect = if dialect_ok {
                format!("ok (control v{CONTROL_VERSION}, protocol v{PROTOCOL_VERSION})")
            } else {
                format!(
                    "MISMATCH (server speaks control v{} protocol v{}, this build \
                     v{CONTROL_VERSION}/v{PROTOCOL_VERSION})",
                    hello.control_version, hello.protocol_version
                )
            };
            rows.push(vec![
                "server".to_string(),
                format!("ok (build {})", hello.build),
            ]);
            rows.push(vec!["dialect".to_string(), dialect]);
            let status = match backend.control(ControlRequest::Status)? {
                ReplyOk::Status(status) => status,
                other => bail!("the server answered Status with {other:?}"),
            };
            rows.push(vec![
                "status".to_string(),
                format!(
                    "pid {}, up {}s, {} panes",
                    status.pid, status.uptime_secs, status.panes
                ),
            ]);
            let routes = match backend.control(ControlRequest::Routes)? {
                ReplyOk::Routes(routes) => routes,
                other => bail!("the server answered Routes with {other:?}"),
            };
            let connected = routes.iter().filter(|r| r.connected).count();
            rows.push(vec![
                "machine links".to_string(),
                format!("{} known, {connected} connected", routes.len()),
            ]);
            // Without hooks an agent reports no status, which means `lyrel
            // agents` shows it standing still and `lyrel wait` never wakes. That
            // failure looks like a hang rather than a missing install, so the
            // check that explains it belongs in the verb people run when
            // something is not working.
            hooks = hook_survey(backend);
            rows.push(vec!["agent hooks".to_string(), hooks_summary(&hooks)]);
            server = json!({
                "reachable": true,
                "dialect_ok": dialect_ok,
                "build": hello.build,
                "status": serde_json::to_value(&status)?,
                "routes": serde_json::to_value(&routes)?,
            });
        }
        Err(e) => {
            rows.push(vec!["server".to_string(), format!("unreachable — {e:#}")]);
        }
    }
    let mut human = output::table(&["CHECK", "RESULT"], &rows);
    if ctx.config_dir.is_none() && ctx.pane.is_none() {
        human.push_str(
            "\nnot inside a lyrel shell — address commands need an explicit %pane/@tab/workspace\n",
        );
    }
    let report = Report {
        human,
        json: json!({
            "context": {
                "config_dir": ctx.config_dir.is_some(),
                "workspace": ctx.ws.is_some(),
                "pane": ctx.pane.is_some(),
            },
            "server": server,
            "hooks": hooks_json(&hooks),
        }),
    };
    if report.json["server"]["reachable"] == false {
        // doctor is the verb people run when something is not working, so an
        // unreachable server is *the* finding — not a row to exit 0 over:
        // `lyrel doctor || alert` has to fire (#592). The table and JSON go
        // out all the same, and stderr carries the headline under `-q`.
        eprintln!("lyrel: doctor: the server is unreachable");
        return Ok(Outcome::Exit(1, report));
    }
    Ok(Outcome::Report(report))
}

/// Where every installable status hook stands on this machine.
///
/// Agents whose state cannot be read at all are left out rather than guessed
/// at: the backend answers `None` both for a `-m` run (hooks are a local
/// install, and this is a local check) and when the app itself cannot be found,
/// and neither is the same as "not installed".
fn hook_survey(backend: &mut dyn Backend) -> Vec<(HookAgent, HooksState)> {
    HookAgent::ALL
        .into_iter()
        .filter_map(|agent| Some((agent, backend.agent_hooks_state(agent)?)))
        .collect()
}

fn hooks_summary(hooks: &[(HookAgent, HooksState)]) -> String {
    if hooks.is_empty() {
        return "unknown — hooks are a local install, and this check could not read them".into();
    }
    let named = |want: HooksState| -> Vec<&'static str> {
        hooks
            .iter()
            .filter(|(_, state)| *state == want)
            .map(|(agent, _)| agent.display_name())
            .collect()
    };
    let installed = named(HooksState::Installed);
    let outdated = named(HooksState::Outdated);

    // The current ones are named because that is the answer to "can I delegate
    // to this agent"; the missing ones are a count, since listing every agent
    // lyrel knows about would bury it. "Up to date" rather than "installed":
    // an outdated hook *is* installed, and saying "none installed" next to six
    // outdated ones reads as a contradiction.
    let mut summary = if installed.is_empty() {
        "none up to date".to_string()
    } else {
        format!("{} up to date", installed.join(", "))
    };
    if !outdated.is_empty() {
        summary.push_str(&format!("; {} OUTDATED", outdated.join(", ")));
    }
    let missing = hooks.len() - installed.len() - outdated.len();
    if missing > 0 {
        summary.push_str(&format!("; {missing} not installed"));
    }
    if installed.is_empty() || !outdated.is_empty() {
        summary.push_str(" (Settings → Integrations)");
    }
    summary
}

fn hooks_json(hooks: &[(HookAgent, HooksState)]) -> Value {
    let slugs = |want: HooksState| -> Vec<&'static str> {
        hooks
            .iter()
            .filter(|(_, state)| *state == want)
            .map(|(agent, _)| agent.slug())
            .collect()
    };
    json!({
        "installed": slugs(HooksState::Installed),
        "outdated": slugs(HooksState::Outdated),
        "not_installed": slugs(HooksState::NotInstalled),
    })
}

#[cfg(test)]
mod tests;
