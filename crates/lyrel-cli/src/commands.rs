use anyhow::Result;
use serde_json::Value;

use crate::address::Context;
use crate::backend::Backend;
use crate::cli::{Cli, Command};

mod agents;
mod capture;
mod diagnostics;
#[cfg(test)]
mod dispatch_tests;
mod execution;
mod launch;
mod pane;
mod send;
mod server;
mod tab;
mod target;
#[cfg(test)]
mod test_support;
mod wait;
mod workspace;
mod worktree;

#[derive(Debug)]
pub struct Report {
    pub human: String,
    pub json: Value,
}

#[derive(Debug)]
pub enum Outcome {
    Report(Report),
    /// A verb that stands in for a child process: the code is the CLI's own
    /// exit status. The report comes too, so `--json` still answers here.
    Exit(i32, Report),
}

fn report(human: impl Into<String>, json: Value) -> Result<Outcome> {
    Ok(Outcome::Report(Report {
        human: human.into(),
        json,
    }))
}

pub fn execute(cli: Cli, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let json_mode = cli.json;
    let machine = cli.machine.clone();
    match cli.command {
        None => launch::execute(cli.path, machine.as_deref(), backend),
        Some(Command::Ls) => workspace::ws_ls(backend),
        Some(Command::Ws(cmd)) => workspace::execute(cmd, ctx, backend),
        Some(Command::New { path, open }) => workspace::new_workspace(path, open, backend),
        Some(Command::Run(args)) => execution::run(args, ctx, backend),
        Some(Command::Split(args)) => pane::pane_split(args, ctx, backend),
        Some(Command::Pane(cmd)) => pane::execute(cmd, ctx, backend),
        Some(Command::Send(args)) => send::send(args, ctx, backend),
        Some(Command::Exec(args)) => execution::exec(args, ctx, backend),
        Some(Command::Capture(args)) => capture::capture(args, ctx, backend),
        Some(Command::Procs { target }) => capture::procs(target.as_deref(), ctx, backend),
        Some(Command::Tab(cmd)) => tab::execute(cmd, ctx, backend),
        Some(Command::Worktree(cmd)) => worktree::execute(cmd, machine.as_deref(), ctx, backend),
        Some(Command::Events) => server::events(json_mode, backend),
        Some(Command::Agents) => agents::agents(backend),
        Some(Command::Wait(args)) => wait::wait(args, ctx, backend),
        Some(Command::Status) => server::status(backend),
        Some(Command::Server(cmd)) => server::execute(cmd, machine.as_deref(), backend),
        Some(Command::Machine(cmd)) => server::machine(cmd, backend),
        Some(Command::Doctor) => diagnostics::doctor(ctx, backend),
    }
}
