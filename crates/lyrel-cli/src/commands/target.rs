use anyhow::{Result, bail};
use lyrel_core::core::machine::Machine;
use lyrel_core::core::session::WorkspaceId;
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};

use crate::address::{self, Context};
use crate::backend::Backend;
use crate::resolve;

pub(super) fn fetch_machine(backend: &mut dyn Backend) -> Result<Machine> {
    match backend.control(ControlRequest::MachineGet)? {
        ReplyOk::MachineTree(m) => Ok(*m),
        other => bail!("the server answered MachineGet with {other:?}"),
    }
}

pub(super) fn resolve_ws(
    explicit: Option<&str>,
    ctx: &Context,
    machine: &Machine,
) -> Result<WorkspaceId> {
    let addr = address::workspace_or_context(explicit, ctx)?;
    Ok(resolve::workspace(machine, &addr)?.id)
}
