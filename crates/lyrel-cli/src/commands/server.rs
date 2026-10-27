use anyhow::{Result, bail};
use lyrel_core::daemon::control::{ControlEvent, ControlRequest, ReplyOk};
use serde_json::{Value, json};

use super::{Outcome, report};
use crate::backend::Backend;
use crate::cli::{MachineCmd, ServerCmd};
use crate::output;

pub(super) fn execute(
    cmd: ServerCmd,
    machine: Option<&str>,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    match cmd {
        ServerCmd::Status => status(backend),
        ServerCmd::Start => local_server(machine, "start", crate::server::start),
        ServerCmd::Stop => local_server(machine, "stop", crate::server::stop),
        ServerCmd::Restart { hard } => {
            local_server(machine, "restart", || crate::server::restart(hard))
        }
        ServerCmd::Logs => local_server(machine, "logs", crate::server::logs),
    }
}

pub(super) fn machine(cmd: MachineCmd, backend: &mut dyn Backend) -> Result<Outcome> {
    match cmd {
        MachineCmd::Ls => machine_ls(backend),
        MachineCmd::Connect { .. } | MachineCmd::Disconnect { .. } => bail!(
            "managing machine links from the CLI is not implemented yet — \
             use the GUI's connection manager for now"
        ),
    }
}

fn local_server(
    machine: Option<&str>,
    verb: &str,
    act: impl FnOnce() -> Result<Outcome>,
) -> Result<Outcome> {
    if let Some(machine) = machine {
        bail!(
            "`lyrel server {verb}` manages only the server on THIS machine — with -m {machine} \
             it would still have acted on the LOCAL server, so it was refused; a remote \
             machine's server lifecycle is handled by the install/reconnect flows, not the CLI"
        );
    }
    act()
}

pub(super) fn events(json_mode: bool, backend: &mut dyn Backend) -> Result<Outcome> {
    backend.events(&mut |event| {
        if json_mode {
            crate::stdio::line(&serde_json::to_string(&event)?);
        } else {
            crate::stdio::line(&event_line(&event));
        }
        Ok(())
    })?;
    report("", Value::Null)
}

fn event_line(event: &ControlEvent) -> String {
    match event {
        ControlEvent::PaneExited { pane_id, code } => match code {
            Some(code) => format!("pane %{pane_id} exited with code {code}"),
            None => format!("pane %{pane_id} exited"),
        },
        ControlEvent::AgentStatus { pane_id, json } => {
            format!("pane %{pane_id} agent status: {json}")
        }
        ControlEvent::Preempted { workspace, by } => {
            format!("workspace {workspace} taken over by {by}")
        }
        ControlEvent::Layout { workspace, delta } => {
            format!("workspace {workspace} layout: {delta:?}")
        }
        ControlEvent::LayoutResync => "layout resync".to_string(),
        other => format!("{other:?}"),
    }
}

pub(super) fn status(backend: &mut dyn Backend) -> Result<Outcome> {
    match backend.control(ControlRequest::Status)? {
        ReplyOk::Status(status) => report(
            output::status_lines(&status),
            serde_json::to_value(&status)?,
        ),
        other => bail!("the server answered Status with {other:?}"),
    }
}

fn machine_ls(backend: &mut dyn Backend) -> Result<Outcome> {
    match backend.control(ControlRequest::Routes)? {
        ReplyOk::Routes(routes) => report(
            output::routes_table(&routes),
            json!({ "machines": serde_json::to_value(&routes)? }),
        ),
        other => bail!("the server answered Routes with {other:?}"),
    }
}

#[cfg(test)]
mod tests;
