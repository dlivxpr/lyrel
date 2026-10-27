use anyhow::{Result, bail};
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::core::session::WorkspaceId;
use lyrel_core::daemon::control::ControlRequest;
use serde_json::json;
use std::time::Duration;

use super::target::fetch_machine;
use super::{Outcome, Report};
use crate::address::{self, Context};
use crate::backend::{Backend, RunSpec};
use crate::cli::{ExecArgs, RunArgs};
use crate::exec::ExecEnd;
use crate::{resolve, screen};

const EXIT_CODE_UNKNOWN: &str = "the command exited but its real exit code could not be determined — exiting 1 as a \
     stand-in, not as the command's own code";

pub(super) fn run(args: RunArgs, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let workspace = match args.ws.as_deref() {
        Some(explicit) => {
            let machine = fetch_machine(backend)?;
            Some(resolve::workspace(&machine, &address::parse_workspace(explicit))?.id)
        }
        None => ctx
            .ws
            .as_deref()
            .and_then(|v| v.parse::<WorkspaceId>().ok()),
    };
    if args.keep && workspace.is_none() {
        bail!(
            "`run --keep` keeps the pane alive, so it must be filed into a workspace — \
             pass --ws, or run inside a lyrel shell where $LYREL_WS names one"
        );
    }
    let pane = backend.run_spawn(RunSpec {
        workspace,
        cwd: args.cwd.clone(),
        command: args.cmd,
        keep: args.keep,
    })?;
    if args.keep {
        let workspace = workspace.expect("checked above: --keep requires a workspace");
        backend.control(ControlRequest::TabCreate {
            workspace,
            at: None,
            pane: PaneSeed {
                pane,
                cwd: args.cwd,
                ssh_spec: None,
                agent: None,
                shell: None,
            },
            tab: None,
        })?;
    }
    let (code, exact) = match backend.run_wait()? {
        Some(code) => (code, true),
        None => {
            eprintln!("lyrel: {EXIT_CODE_UNKNOWN}");
            (1, false)
        }
    };
    // The command's own output already went to stdout as it streamed, so the
    // human report is empty — but --json still owes the caller a machine
    // readable answer, and `exit_code_known` is how it tells a real 1 from the
    // stand-in above.
    Ok(Outcome::Exit(
        code,
        Report {
            human: String::new(),
            json: json!({
                "pane": pane,
                "exit": code,
                "exit_code_known": exact,
                "kept": args.keep,
            }),
        },
    ))
}

pub(super) fn exec(args: ExecArgs, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let pane = address::pane_or_context(args.target.as_deref(), ctx)?;
    let line = args.cmd.join(" ");
    if line.trim().is_empty() {
        bail!("exec needs a command after `--`");
    }
    // The line is typed, so every byte of it goes through the shell's line
    // editor: a newline would submit half of it and a Tab would complete. One
    // line of printable text is the only thing that runs as written — the same
    // test the GUI applies before it types a line rather than pasting it.
    if line.chars().any(char::is_control) {
        bail!(
            "exec types one line at the shell's prompt, and this one holds a control \
             character (a newline, a tab, an escape) the line editor would act on — join \
             the commands with `;` or `&&`, or put them in a script and exec that"
        );
    }
    let mut wire = line.clone().into_bytes();
    wire.push(b'\r');
    let started = std::time::Instant::now();
    let run = backend.exec(pane, wire, args.timeout.map(Duration::from_secs))?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    // The caller's own pane is the one pane that can never be at a prompt
    // while this runs: its shell is busy running `lyrel exec`.
    let own = ctx
        .pane
        .as_deref()
        .and_then(|p| address::parse_pane(p).ok())
        == Some(pane);
    match run.end {
        ExecEnd::NoIntegration => bail!(
            "pane %{pane} has sent no shell-integration prompt marks, so nothing would say \
             when a command there ends — exec needs lyrel's shell integration loaded in the \
             pane's shell (`lyrel doctor`); without it, `send … --enter` and \
             `wait --until free` are the way"
        ),
        ExecEnd::Busy if own => bail!(
            "pane %{pane} is this shell, and it is busy running this very command — exec \
             into another pane"
        ),
        ExecEnd::Busy => bail!(
            "pane %{pane} is not at a shell prompt — something is already running there. \
             `lyrel wait %{pane} --until free` waits for it"
        ),
        _ => {}
    }

    let text = if args.raw {
        String::from_utf8_lossy(&run.output).into_owned()
    } else {
        screen::render(&[crate::backend::CaptureSegment {
            size: run.size,
            bytes: run.output.clone(),
        }])
    };
    if run.dropped > 0 {
        eprintln!(
            "lyrel: exec %{pane}: the first {} bytes of output were dropped to keep the rest \
             in memory — the pane's own scrollback may still hold them",
            run.dropped
        );
    }
    let (code, known, timed_out) = match run.end {
        ExecEnd::Finished {
            exit: Some(code),
            ran: true,
        } => (code, true, false),
        ExecEnd::Finished {
            exit: None,
            ran: true,
        } => {
            eprintln!("lyrel: {EXIT_CODE_UNKNOWN}");
            (1, false, false)
        }
        ExecEnd::Finished { ran: false, .. } => {
            eprintln!(
                "lyrel: pane %{pane}: the shell came back to its prompt without running the \
                 line — a syntax error, most likely; what it printed is the output"
            );
            (1, false, false)
        }
        ExecEnd::TimedOut { started: true } => {
            eprintln!(
                "lyrel: pane %{pane}: still running after {}s — timed out, and left running \
                 (`lyrel send %{pane} --key C-c` interrupts it)",
                args.timeout.unwrap_or_default()
            );
            (124, false, true)
        }
        ExecEnd::TimedOut { started: false } => {
            eprintln!(
                "lyrel: pane %{pane}: timed out after {}s, and the shell never started the \
                 line — it is most likely waiting on a continuation prompt (an unclosed \
                 quote or bracket); `lyrel send %{pane} --key C-c` abandons it",
                args.timeout.unwrap_or_default()
            );
            (124, false, true)
        }
        ExecEnd::PaneExited(_) => {
            eprintln!("lyrel: pane %{pane} exited before its shell came back to a prompt");
            (1, false, false)
        }
        ExecEnd::NoIntegration | ExecEnd::Busy => unreachable!("refused above"),
    };
    // `-q` keeps the exit code and drops the output, which is what the global
    // flag already does to any report.
    Ok(Outcome::Exit(
        code,
        Report {
            human: text.clone(),
            json: json!({
                "pane": pane,
                "command": line,
                "exit": code,
                "exit_code_known": known,
                "timed_out": timed_out,
                "pane_exited": matches!(run.end, ExecEnd::PaneExited(_)),
                "output": text,
                "bytes": run.output.len(),
                "elapsed_ms": elapsed_ms,
            }),
        },
    ))
}

#[cfg(test)]
mod tests;
