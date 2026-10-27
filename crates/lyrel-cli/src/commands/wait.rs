use anyhow::{Result, bail};
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::json;

use super::target::fetch_machine;
use super::{Outcome, Report, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::{WaitArgs, WaitState};

/// The one verb that *blocks*: poll until the watched pane reaches a requested
/// state, then report it. This is what turns the CLI into an orchestration tool
/// — "wake me when my peer agent needs input, or finishes its turn" — without
/// the screen-scraping a tmux-based agent team resorts to.
///
/// Two kinds of pane can be waited on, and they are watched differently. An
/// agent pane has a status the server keeps from hook events; a pane merely
/// running a command has none, and for it the question is whether the
/// foreground command has exited — `free`, read off the process tree. Keeping
/// both here rather than in two verbs means a caller that does not know which
/// kind it has can ask for `waiting,done,free,exit` and get an answer either
/// way.
///
/// A poll rather than an `events` subscription on purpose: a one-shot,
/// stateless question composes into scripts (`lyrel wait %3 && lyrel capture %3
/// --plain`), survives a server restart mid-wait, and needs no cursor
/// management. At the default 500ms interval an agent wait costs one aggregate
/// control request per tick — the same request `lyrel agents` makes once.
pub(super) fn wait(args: WaitArgs, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    use lyrel_core::core::cli_agent::AgentStatus;
    use std::time::{Duration, Instant};

    /// How many polls may pass before liveness is re-checked. The agent
    /// snapshot carries no liveness of its own and the daemon keeps a dead
    /// pane in its registry until it is closed, so an agent that died
    /// mid-turn would otherwise report `working` until the timeout. Every
    /// few polls is cheap; the fast path — the first poll already answers —
    /// still costs exactly one request.
    const LIVENESS_EVERY: u32 = 4;

    /// Where the pane stood the moment we looked. `None` is "no agent state
    /// at all", which is itself a position: an agent appearing is a change.
    type Cursor = Option<(AgentStatus, u64)>;

    /// How many polls in a row must fail to determine freeness before the wait
    /// calls it structural and stops.
    ///
    /// One is not enough: a pane whose `ssh` handshake is still in flight has
    /// no far-side prompt mark yet and no local tree worth reading, and that
    /// window is shorter than a poll. Two consecutive misses is one `--interval`
    /// of grace — long enough for the transient, short enough that a wait with
    /// no `--timeout` at all still ends rather than hanging on a question this
    /// machine cannot answer.
    const UNKNOWN_POLLS: u32 = 2;

    let pane = address::pane_or_context(args.target.as_deref(), ctx)?;
    // `checked_add` rather than `+`: an absurd `--timeout` must not panic.
    let deadline = args
        .timeout
        .and_then(|t| Instant::now().checked_add(Duration::from_secs(t)));
    let interval = Duration::from_millis(args.interval);
    let watch_free = args.until.contains(&WaitState::Free);
    // `free` is the only state this wait computes rather than reads. When it is
    // also the only thing that could still answer, an undeterminable freeness
    // ends the wait instead of riding out the deadline — `exit` does not count,
    // because it arrives on its own whether it was asked for or not.
    let free_is_the_only_hope = watch_free
        && args
            .until
            .iter()
            .all(|s| matches!(s, WaitState::Free | WaitState::Exit));
    let mut baseline: Option<Cursor> = None;
    // Sticky: has the pane been seen running something since the wait began?
    // This is `--changed`'s edge for `free` — see the flag's own comment.
    let mut seen_busy = false;
    // Why freeness could not be read, and for how many polls running. Cleared
    // the moment a verdict does arrive: a wait that timed out on a pane it
    // could read perfectly well by then must not blame the handshake it
    // watched go by.
    let mut unknown: Option<String> = None;
    let mut unknown_polls: u32 = 0;
    let mut polls: u32 = 0;
    loop {
        let states = match backend.control(ControlRequest::AgentStates)? {
            ReplyOk::AgentStates(states) => states,
            other => bail!("the server answered AgentStates with {other:?}"),
        };
        let entry = states.into_iter().find(|s| s.pane_id == pane);
        let mut current = match &entry {
            Some(e) => match e.state.status {
                AgentStatus::Idle => WaitState::Idle,
                AgentStatus::Working => WaitState::Working,
                AgentStatus::Waiting => WaitState::Waiting,
                AgentStatus::Done => WaitState::Done,
            },
            // No agent state for the pane: a live one is agentless — a plain
            // shell, or an agent whose hooks never got installed — and a dead
            // or vanished one has exited. Reporting `idle` here (as this once
            // did) made `--until idle` answer "finished" about a pane that was
            // midway through a build. The machine tree is only fetched on this
            // branch — while an agent is reporting, its state alone answers.
            None if pane_is_live(backend, pane)? => WaitState::NoAgent,
            None => WaitState::Exit,
        };

        // Status is a level, not an edge (see `--changed` in cli.rs): the
        // position we arrived at is last turn's answer until the agent moves.
        let cursor: Cursor = entry.as_ref().map(|e| (e.state.status, e.state.activity));
        let baseline = *baseline.get_or_insert(cursor);
        let mut changed = cursor != baseline;

        // `free` is a fact about the pane, not the agent ladder, so it is asked
        // separately, only when requested, and only once the ladder has failed
        // to answer. A state the caller listed is their answer: overwriting a
        // real `waiting` with a process-tree fact would strand a pane whose
        // depth-0 process *is* the agent (see `pane_freeness`), where the tree
        // reads free for the whole turn.
        if watch_free && current != WaitState::Exit && !args.until.contains(&current) {
            match pane_freeness(backend, pane)? {
                Freeness::Free => {
                    current = WaitState::Free;
                    changed = seen_busy;
                    (unknown, unknown_polls) = (None, 0);
                }
                Freeness::Busy => {
                    seen_busy = true;
                    (unknown, unknown_polls) = (None, 0);
                }
                // Not `seen_busy`: "we could not look" is not "something was
                // running", and letting it set that flag was what suppressed
                // the one hint that would have pointed at the gap (#840).
                Freeness::Unknown(why) => {
                    unknown_polls += 1;
                    unknown = Some(why);
                }
            }
        }

        let mut matched = args.until.contains(&current) && (changed || !args.changed);
        polls += 1;
        // A reporting agent can outlive its pane; re-check on a throttle so a
        // crashed worker ends the wait instead of spinning on a stale status.
        if !matched
            && entry.is_some()
            && polls.is_multiple_of(LIVENESS_EVERY)
            && !pane_is_live(backend, pane)?
        {
            current = WaitState::Exit;
            matched = args.until.contains(&current);
        }

        // Exit ends every wait, requested or not: whatever the caller was
        // waiting for can no longer happen, and reporting beats spinning
        // forever on a ghost. `--changed` does not veto it either — a pane
        // that was already dead is not going to move.
        if matched || current == WaitState::Exit {
            let session = entry.as_ref().map(|e| &e.state);
            let json = json!({
                "pane": pane,
                "status": current.name(),
                "matched": matched,
                // False means the pane moved into this state while we watched;
                // true means it was already there when the wait began, i.e.
                // the answer may belong to a previous turn.
                "stale": !changed,
                "activity": session.map(|s| s.activity),
                "message": session.and_then(|s| s.message.clone()),
                "session_id": session.and_then(|s| s.session_id.clone()),
            });
            if !matched {
                // Structured even here: a script has to tell "my peer died"
                // apart from "the daemon is unreachable", and an anyhow error
                // would leave --json with nothing to read.
                //
                // The headline goes to stderr all the same, so `-q` still
                // reports it — the discipline `pane close` set: a failure is
                // not "output on success", and an exit code alone says which
                // wait died nowhere (#590).
                eprintln!("lyrel: pane %{pane} exited before reaching the awaited state");
                return Ok(Outcome::Exit(
                    1,
                    Report {
                        human: format!("pane %{pane} exited before reaching the awaited state"),
                        json,
                    },
                ));
            }
            let mut human = format!("pane %{pane}: {}", current.name());
            if let Some(msg) = session.and_then(|s| s.message.as_deref()) {
                human.push_str(&format!(" — {msg}"));
            }
            if !changed {
                human.push_str(match current {
                    WaitState::Free => " (already free — nothing ran while we watched)",
                    _ => " (unchanged since the wait began)",
                });
            }
            return report(human, json);
        }

        // Nothing here can answer, and `free` was the only thing left that
        // could. Say so now: polling harder cannot turn "we cannot see the far
        // side" into a verdict, and a wait with no `--timeout` would otherwise
        // sit on this question until the caller killed it.
        if free_is_the_only_hope && unknown_polls >= UNKNOWN_POLLS {
            let why = unknown.as_deref().unwrap_or("no reason recorded");
            let human = format!("pane %{pane}: cannot determine whether it is free — {why}");
            eprintln!("lyrel: pane %{pane}: cannot determine whether it is free");
            let session = entry.as_ref().map(|e| &e.state);
            return Ok(Outcome::Exit(
                1,
                Report {
                    human,
                    // The success path's shape, so a consumer written against
                    // it does not find its fields missing on the one branch it
                    // wrote error handling for (#589) — plus the reason, which
                    // is the only thing this branch actually knows.
                    json: json!({
                        "pane": pane,
                        "status": WaitState::Unknown.name(),
                        "matched": false,
                        "stale": !changed,
                        "free_unknown": why,
                        "activity": session.map(|s| s.activity),
                        "message": session.and_then(|s| s.message.clone()),
                        "session_id": session.and_then(|s| s.session_id.clone()),
                    }),
                },
            ));
        }

        if deadline.is_some_and(|d| Instant::now() >= d) {
            // 124 = the `timeout(1)` convention: "gave up", distinct from
            // both success and error, so orchestration scripts can branch.
            let mut human = format!("pane %{pane}: still {} — timed out", current.name());
            // A wait for agent states that never move is the shape of both
            // "there is no agent here" and "the agent's hooks are missing",
            // and neither is visible from a timeout alone. Say which door to
            // try rather than leaving the caller to poll harder.
            //
            // Only for a caller who has not already tried that door. Sending
            // someone back to `--until free` when `--until free` is what just
            // timed out is the part of this message that cost an agent a
            // session (#840); when they did ask, the hint below carries the
            // reason freeness never answered instead.
            if current == WaitState::NoAgent && !watch_free {
                human.push_str(
                    "\nnothing is reporting agent status in this pane — for a plain command \
                     wait `--until free`, and for an agent check `lyrel agents` for a missing \
                     status hook",
                );
            }
            // Freeness was asked for and never came back with a verdict. This
            // is the whole answer to "why did nothing happen for --timeout
            // seconds", so it goes first among the `free` hints.
            if let Some(why) = &unknown {
                human.push_str(&format!(
                    "\ncould not determine whether this pane is free — {why}"
                ));
            }
            // `--changed` needs to have *seen* the pane busy, and a command
            // that starts and finishes inside one interval never is. That
            // looks exactly like "the command never ran", so say both, rather
            // than let a finished command read as a timeout.
            if current == WaitState::Free && !seen_busy {
                human.push_str(
                    "\nnothing was ever seen running here — either the command never started, \
                     or it finished inside one --interval. Poll faster (--interval 100) or drop \
                     --changed",
                );
            }
            // The headline goes to stderr all the same, so `-q` still
            // reports it — see the sibling exit above for why (#590).
            eprintln!("lyrel: pane %{pane}: still {} — timed out", current.name());
            return Ok(Outcome::Exit(
                124,
                Report {
                    human,
                    // The same shape as a finished wait, plus the flag that
                    // says the deadline ended it: a consumer written against
                    // the success path must not find its fields missing on
                    // exactly the branch it wrote error handling for (#589).
                    json: {
                        let session = entry.as_ref().map(|e| &e.state);
                        json!({
                            "pane": pane,
                            "status": current.name(),
                            "matched": false,
                            "stale": !changed,
                            "timed_out": true,
                            // Present only when `free` was asked for and never
                            // resolved. A script that reads it knows the
                            // timeout says nothing about the pane.
                            "free_unknown": unknown,
                            "activity": session.map(|s| s.activity),
                            "message": session.and_then(|s| s.message.clone()),
                            "session_id": session.and_then(|s| s.session_id.clone()),
                        })
                    },
                },
            ));
        }
        // Never sleep past the deadline: a long `--interval` must not turn a
        // short `--timeout` into a long one.
        let nap = match deadline {
            Some(d) => interval.min(d.saturating_duration_since(Instant::now())),
            None => interval,
        };
        std::thread::sleep(nap);
    }
}

/// Whether the pane is back to its bare shell — nothing running in front of it.
///
/// Three answers, not two. `Busy` and `Unknown` used to be the same `false`,
/// and that is the whole of #840: on a pane where the question is structurally
/// unanswerable the wait read "still busy" on every poll, rode the full
/// `--timeout`, and then recommended the flag that had just failed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Freeness {
    Free,
    Busy,
    /// Nothing here can answer. The string is the reason, written for the
    /// caller: it is what the wait prints instead of a timeout.
    Unknown(String),
}

/// Read the pane's freeness.
///
/// Two sources, and which one leads depends on where the pane's session is.
///
/// **The process tree**, for a pane whose pty is on this machine. Depth, not
/// count: the pane's own shell sits at depth 0 and everything it launched hangs
/// below, so "nothing deeper than the shell" holds however many shells the pane
/// ended up with, and does not have to guess at process names. It is also the
/// portable question — Windows has no foreground process group to ask about, so
/// `ProcEntry::foreground` is never true there. What it cannot see, both by
/// construction: a pane whose depth-0 process *is* the command — which is what
/// `lyrel run` spawns — reads free for as long as it runs, and a backgrounded
/// job keeps a pane busy after the foreground command is long gone.
///
/// **The shell's own OSC 133 marks**, which are the only thing that can speak
/// for a pane that is the near end of a connection. There the local tree
/// describes the tunnel: a native-SSH pane has no local tree at all, and a pane
/// running `ssh` has one whose depth-1 process is the `ssh` itself, busy for as
/// long as you are logged in. A prompt mark on such a pane can only have come
/// from the far shell — the near one cannot be at a prompt while the connection
/// owns its pty — so it is both trustworthy and the only evidence available.
///
/// Where they disagree on a local pane, a prompt mark outranks a deeper
/// process, because the process drawing that prompt is a shell: a `sudo -i`, a
/// nested `bash`, an `ssh` on a platform where the daemon cannot name it as
/// remote. It does not work the other way round — the absence of a mark proves
/// nothing, and the tree keeps the verdict there.
fn pane_freeness(backend: &mut dyn Backend, pane: u64) -> Result<Freeness> {
    let answer = backend.procs(pane)?;
    let bare_tree = !answer.procs.is_empty() && answer.procs.iter().all(|p| p.depth == 0);
    let Some(ctx) = &answer.context else {
        // A daemon from before the field existed. It can only be answering for
        // a pty of its own, so the tree is all there was and all there is.
        return Ok(match (bare_tree, answer.procs.is_empty()) {
            (true, _) => Freeness::Free,
            (false, false) => Freeness::Busy,
            (false, true) => Freeness::Unknown(
                "this server is too old to say where the pane's session lives, and its \
                 process tree came back empty"
                    .into(),
            ),
        });
    };

    let remote = ctx.remote.as_ref();
    if remote.is_some() || !ctx.local_pty {
        let whereabouts = match remote {
            Some(r) => format!("connected to {}", r.target),
            // A pane with no pty here and no context to name: the daemon knows
            // the session is elsewhere without knowing where.
            None => "not backed by a pty on this machine".to_string(),
        };
        return Ok(match ctx.at_prompt {
            // Only the far shell can be at a prompt while the near end is busy
            // holding the connection open.
            Some(true) => Freeness::Free,
            // The connection is over and the near shell is bare again — the
            // remote context just has not been re-probed yet.
            _ if ctx.local_pty && bare_tree => Freeness::Free,
            // Once the far side has proved it reports, its silence means work.
            Some(false) if ctx.remote_prompt_seen => Freeness::Busy,
            _ => Freeness::Unknown(format!(
                "this pane is {whereabouts}, so its local process tree describes this end of \
                 the connection, not what is running on the far one — and the far shell has \
                 sent no prompt mark, so nothing here can tell an idle remote prompt from a \
                 running remote command. Install lyrel's shell integration on the remote host, \
                 or wait on something observable from here (`--until exit`, or an agent status)"
            )),
        });
    }

    // A local pane. The tree leads; a prompt mark only ever adds to it.
    if bare_tree || ctx.at_prompt == Some(true) {
        return Ok(Freeness::Free);
    }
    if !answer.procs.is_empty() || ctx.at_prompt == Some(false) {
        return Ok(Freeness::Busy);
    }
    Ok(Freeness::Unknown(
        "the pane's process tree came back empty and its shell has sent no prompt marks, so \
         there is nothing here to read freeness off — check `lyrel procs` on this pane"
            .into(),
    ))
}

/// Whether the daemon still has a live pane behind this id. Absent from the
/// tree counts as dead: a closed pane is as gone as an exited one.
fn pane_is_live(backend: &mut dyn Backend, pane: u64) -> Result<bool> {
    Ok(fetch_machine(backend)?
        .panes
        .iter()
        .any(|p| p.id == pane && p.live))
}

#[cfg(test)]
mod tests;
