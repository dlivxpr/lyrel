use crate::address::Context;
use crate::commands::Outcome;
use crate::commands::execute;
use crate::commands::test_support::agent_state;
use crate::commands::test_support::agent_state_at;
use crate::commands::test_support::cli;
use crate::commands::test_support::human;
use crate::commands::test_support::json_of;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;
use serde_json::json;

/// A pane sitting at its prompt: the shell, and nothing in front of it.
fn idle_procs() -> lyrel_core::daemon::protocol::PaneProcs {
    lyrel_core::daemon::protocol::PaneProcs {
        procs: vec![proc_entry(100, "zsh", 0, true)],
        ports: Vec::new(),
        probe: Default::default(),
        context: Some(local_context(None)),
    }
}

/// The same pane with a command running in it.
fn busy_procs() -> lyrel_core::daemon::protocol::PaneProcs {
    lyrel_core::daemon::protocol::PaneProcs {
        procs: vec![
            proc_entry(100, "zsh", 0, false),
            proc_entry(101, "cargo", 1, true),
        ],
        ports: Vec::new(),
        probe: Default::default(),
        context: Some(local_context(None)),
    }
}

/// A pane whose pty is on this machine, optionally with a shell that is
/// reporting prompt marks.
fn local_context(at_prompt: Option<bool>) -> lyrel_core::daemon::protocol::PaneContext {
    lyrel_core::daemon::protocol::PaneContext {
        remote: None,
        local_pty: true,
        at_prompt,
        remote_prompt_seen: false,
        bracketed_paste: None,
    }
}

/// A pane whose shell is running `ssh`: the local tree has the connection
/// in it, and everything that matters is on the far end.
fn ssh_procs(
    at_prompt: Option<bool>,
    remote_prompt_seen: bool,
) -> lyrel_core::daemon::protocol::PaneProcs {
    use lyrel_core::daemon::protocol::{RemoteContext, RemoteKind};
    lyrel_core::daemon::protocol::PaneProcs {
        procs: vec![
            proc_entry(100, "zsh", 0, false),
            proc_entry(101, "ssh", 1, true),
        ],
        ports: Vec::new(),
        probe: Default::default(),
        context: Some(lyrel_core::daemon::protocol::PaneContext {
            remote: Some(RemoteContext {
                kind: RemoteKind::Ssh,
                argv: vec!["ssh".into(), "build-box".into()],
                target: "build-box".into(),
            }),
            local_pty: true,
            at_prompt,
            remote_prompt_seen,
            bracketed_paste: None,
        }),
    }
}

/// A pane routed to a remote lyrel daemon: no pty on this machine at all,
/// so the process list is empty by construction rather than by failure.
fn routed_procs(at_prompt: Option<bool>) -> lyrel_core::daemon::protocol::PaneProcs {
    use lyrel_core::daemon::protocol::{RemoteContext, RemoteKind};
    lyrel_core::daemon::protocol::PaneProcs {
        procs: Vec::new(),
        ports: Vec::new(),
        probe: Default::default(),
        context: Some(lyrel_core::daemon::protocol::PaneContext {
            remote: Some(RemoteContext {
                kind: RemoteKind::NativeSsh,
                argv: Vec::new(),
                target: "me@build-box".into(),
            }),
            local_pty: false,
            at_prompt,
            remote_prompt_seen: at_prompt == Some(true),
            bracketed_paste: None,
        }),
    }
}

fn proc_entry(
    pid: u32,
    name: &str,
    depth: u8,
    foreground: bool,
) -> lyrel_core::daemon::protocol::ProcEntry {
    lyrel_core::daemon::protocol::ProcEntry {
        pid,
        name: name.into(),
        depth,
        foreground,
        ..Default::default()
    }
}

/// The happy path is one aggregate poll: a matching agent state answers
/// immediately, carrying the event's message and native session id — the
/// two things an orchestrator needs to act on the wake-up.
#[test]
fn wait_returns_the_moment_the_state_matches() {
    use lyrel_core::core::cli_agent::AgentStatus;
    let mut backend = mock();
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state(
            3,
            AgentStatus::Waiting,
        )]));
    let out = run_cli(&["lyrel", "wait", "%3"], &Context::default(), &mut backend);
    let json = json_of(out);
    assert_eq!(json["status"], "waiting");
    assert_eq!(json["matched"], true);
    assert_eq!(json["message"], "needs permission");
    assert_eq!(json["session_id"], "sess-9");
    // …and flagged as a state we merely walked in on, not one we watched
    // the pane move into: it may be answering a previous turn.
    assert_eq!(json["stale"], true);
    // The machine tree was never consulted — the agent state alone answered.
    assert_eq!(backend.control_calls, vec![ControlRequest::AgentStates]);
}

/// `--changed` is the fix for a level-triggered status: right after a
/// `send`, the agent still reports last turn's state. The flag refuses the
/// position the wait arrived at and wakes only once the pane moves.
#[test]
fn wait_changed_refuses_the_state_it_arrived_in() {
    use lyrel_core::core::cli_agent::AgentStatus;

    // Already `waiting` when the wait began, and it never moves: timeout,
    // not a bogus wake-up carrying the previous turn's message.
    let mut backend = mock();
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state(
            3,
            AgentStatus::Waiting,
        )]));
    let out = execute(
        cli(&[
            "lyrel",
            "wait",
            "%3",
            "--until",
            "waiting",
            "--changed",
            "--timeout",
            "0",
        ]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    assert!(
        matches!(out, Outcome::Exit(124, _)),
        "a state that was already standing must not satisfy --changed"
    );

    // The same state again, but this time the agent moved under it: the
    // activity counter ticks even when the status letter does not.
    let mut backend = mock();
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state_at(
            3,
            AgentStatus::Waiting,
            0,
        )]));
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state_at(
            3,
            AgentStatus::Waiting,
            1,
        )]));
    let json = json_of(run_cli(
        &[
            "lyrel",
            "wait",
            "%3",
            "--until",
            "waiting",
            "--changed",
            "--interval",
            "50",
        ],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "waiting");
    assert_eq!(json["matched"], true);
    assert_eq!(json["stale"], false, "this one we watched it move into");
    assert_eq!(json["activity"], 1);
}

/// An agent state outlives the pane's child — the daemon keeps a dead pane
/// registered until it is closed. Without a liveness re-check a crashed
/// worker would report `working` right up to the timeout.
#[test]
fn wait_ends_when_a_reporting_agents_pane_dies() {
    use lyrel_core::core::cli_agent::AgentStatus;
    let mut backend = mock();
    for p in &mut backend.machine.panes {
        if p.id == 3 {
            p.live = false;
        }
    }
    for _ in 0..4 {
        backend
            .replies
            .push_back(ReplyOk::AgentStates(vec![agent_state(
                3,
                AgentStatus::Working,
            )]));
    }
    let json = json_of(run_cli(
        &["lyrel", "wait", "%3", "--interval", "50"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "exit");
    assert_eq!(json["matched"], true, "the default until-set covers exit");
    assert!(
        backend.control_calls.contains(&ControlRequest::MachineGet),
        "liveness has to come from the tree; the agent snapshot has none"
    );
}

/// Panes without an agent state fall back to the machine tree: live means
/// `no-agent`, dead-or-gone means exit — which ends every wait, but only
/// counts as *matched* when the caller listed it.
#[test]
fn wait_reads_agentless_panes_from_the_tree() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    let out = run_cli(
        &["lyrel", "wait", "%3", "--until", "no-agent"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(json_of(out)["status"], "no-agent");

    // Pane 9 exists nowhere: "exit", matched by the default until-set.
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    let out = run_cli(&["lyrel", "wait", "%9"], &Context::default(), &mut backend);
    let json = json_of(out);
    assert_eq!(json["status"], "exit");
    assert_eq!(json["matched"], true);

    // Waiting for a state a dead pane can never reach is a failure, not a
    // silent success — but a *structured* one: a script has to tell "my
    // peer died" apart from "the daemon is unreachable", and an anyhow
    // error would leave --json with nothing to read.
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    let out = execute(
        cli(&["lyrel", "wait", "%9", "--until", "done"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a dead pane is an exit code with a report, not a bare error");
    let Outcome::Exit(1, r) = out else {
        panic!("a dead pane that cannot reach `done` must exit 1 with its report");
    };
    assert_eq!(r.json["status"], "exit");
    assert_eq!(r.json["matched"], false);
    assert!(r.human.contains("exited"), "{}", r.human);
}

/// The trap this state exists to close. A pane with nothing reporting used
/// to answer `idle`, so `--until idle` returned success — instantly, with
/// `matched: true` — about a shell that was midway through a build. The
/// caller then read a half-finished screen and believed it.
#[test]
fn wait_does_not_call_a_busy_shell_idle() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    backend.procs_reply = busy_procs();

    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "idle", "--timeout", "0"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    let Outcome::Exit(124, r) = out else {
        panic!("a pane with no agent must not satisfy --until idle");
    };
    assert_eq!(r.json["status"], "no-agent");
    assert!(
        r.human.contains("--until free"),
        "the timeout should point at the flag that answers this question: {}",
        r.human
    );
}

/// `free` is the missing half of the verb: an agent pane has a status to
/// wait on, a pane merely running a command has only its process tree.
/// Nothing below the depth-0 shell means the foreground command exited.
#[test]
fn wait_free_ends_when_the_foreground_command_exits() {
    let mut backend = mock();
    for _ in 0..3 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    // Busy, busy, then back to the bare shell.
    backend.procs_replies.push_back(busy_procs());
    backend.procs_replies.push_back(busy_procs());
    backend.procs_replies.push_back(idle_procs());

    let json = json_of(run_cli(
        &["lyrel", "wait", "%3", "--until", "free", "--interval", "50"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "free");
    assert_eq!(json["matched"], true);
    assert_eq!(json["stale"], false, "we watched the command finish");
    assert_eq!(
        backend.procs_calls.len(),
        3,
        "one process-tree read per poll, and only because `free` was asked for"
    );
}

/// The process tree is level-triggered like the agent ladder, but a shell
/// that goes free → busy → free lands back where it started, so a baseline
/// comparison would miss it. `--changed` therefore means "something ran
/// while I watched" here — which is what a caller wants right after `send`.
#[test]
fn wait_changed_free_waits_for_something_to_actually_run() {
    // Already free and it stays that way: the command has not started yet,
    // so answering "free" would report the shell we sent the work *to*.
    let mut backend = mock();
    for _ in 0..2 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    backend.procs_reply = idle_procs();
    let out = execute(
        cli(&[
            "lyrel",
            "wait",
            "%3",
            "--until",
            "free",
            "--changed",
            "--timeout",
            "0",
        ]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    let Outcome::Exit(124, r) = out else {
        panic!("a pane that was free all along has not run anything");
    };
    // A timeout answers in the success path's own shape, plus the flag —
    // a consumer's error branch must not meet missing fields (#589).
    assert_eq!(r.json["timed_out"], true);
    assert_eq!(r.json["matched"], false);
    assert_eq!(r.json["stale"], true, "nothing ran while we watched");
    assert!(r.json.get("session_id").is_some());

    // Free → busy → free is the real shape, and it must wake.
    let mut backend = mock();
    for _ in 0..3 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    backend.procs_replies.push_back(idle_procs());
    backend.procs_replies.push_back(busy_procs());
    backend.procs_replies.push_back(idle_procs());
    let json = json_of(run_cli(
        &[
            "lyrel",
            "wait",
            "%3",
            "--until",
            "free",
            "--changed",
            "--interval",
            "50",
        ],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "free");
    assert_eq!(json["matched"], true);
    assert_eq!(json["stale"], false);
}

/// A command that starts and finishes between two polls is never *seen*
/// busy, which is indistinguishable from one that never ran — so the
/// timeout has to name both doors instead of letting a finished command
/// read as "still going".
#[test]
fn wait_changed_free_says_why_it_saw_nothing_run() {
    let mut backend = mock();
    for _ in 0..2 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    backend.procs_reply = idle_procs();

    let out = execute(
        cli(&[
            "lyrel",
            "wait",
            "%3",
            "--until",
            "free",
            "--changed",
            "--timeout",
            "0",
        ]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    let Outcome::Exit(124, r) = out else {
        panic!("a pane that was free all along has not run anything");
    };
    assert!(
        r.human.contains("--interval") && r.human.contains("--changed"),
        "the timeout should name the two ways out: {}",
        r.human
    );
}

/// `free` answers for a pane the agent ladder cannot, so it must not answer
/// *over* it. A pane whose depth-0 process is the agent itself reads free
/// for its whole turn; letting that outrank a `waiting` the caller asked
/// for would strand exactly the delegation loop the verb exists for.
#[test]
fn wait_free_does_not_overrule_a_state_the_caller_asked_for() {
    use lyrel_core::core::cli_agent::AgentStatus;
    let mut backend = mock();
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state(
            3,
            AgentStatus::Waiting,
        )]));
    // The agent is the pane's only process, so the tree reads "free".
    backend.procs_reply = idle_procs();

    let json = json_of(run_cli(
        &["lyrel", "wait", "%3", "--until", "waiting,free"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "waiting", "the ladder answered first");
    assert_eq!(json["matched"], true);
    assert!(
        backend.procs_calls.is_empty(),
        "and the process tree was never asked"
    );
}

/// An unreadable process tree is not an idle one. Answering `free` on an
/// empty reply would be the same false success `no-agent` was added to
/// remove, one layer down.
///
/// A bare `PaneProcs` is also what a server from before `context` existed
/// sends, so this pins the fallback: no context means the tree was all
/// there ever was, and an empty one is still "we could not look".
#[test]
fn wait_free_does_not_read_an_empty_process_tree_as_finished() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    backend.procs_reply = lyrel_core::daemon::protocol::PaneProcs::default();

    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "free", "--timeout", "0"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    assert!(
        matches!(out, Outcome::Exit(124, _)),
        "nothing was seen, so nothing can be claimed"
    );

    // Without a deadline to hide behind it says so rather than spinning.
    let mut backend = mock();
    for _ in 0..4 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    backend.procs_reply = lyrel_core::daemon::protocol::PaneProcs::default();
    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "free", "--interval", "50"]),
        &Context::default(),
        &mut backend,
    )
    .expect("an undeterminable pane is an exit code, not an error");
    let Outcome::Exit(1, r) = out else {
        panic!("expected exit 1 with a reason, got {out:?}");
    };
    assert_eq!(r.json["status"], "unknown");
    assert!(
        r.json["free_unknown"]
            .as_str()
            .is_some_and(|w| w.contains("too old")),
        "an old server's silence is named as such: {:?}",
        r.json["free_unknown"]
    );
}

/// The heart of #840. A pane sitting at an idle prompt over `ssh` has a
/// local process tree that is busy for as long as you are logged in — the
/// `ssh` itself — so the tree can never report the pane free. The far
/// shell's own prompt mark can, and it is trustworthy precisely because
/// the near shell cannot be at a prompt while the connection holds its pty.
#[test]
fn wait_free_answers_from_the_far_shells_prompt_on_an_ssh_pane() {
    let mut backend = mock();
    for _ in 0..3 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    // The tree is identical on all three polls — `ssh` at depth 1 — and
    // only the marks move.
    backend
        .procs_replies
        .push_back(ssh_procs(Some(false), true));
    backend
        .procs_replies
        .push_back(ssh_procs(Some(false), true));
    backend.procs_replies.push_back(ssh_procs(Some(true), true));

    let json = json_of(run_cli(
        &["lyrel", "wait", "%3", "--until", "free", "--interval", "50"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "free");
    assert_eq!(json["matched"], true);
    assert_eq!(json["stale"], false, "we watched the remote command finish");
}

/// A pane routed to a remote daemon has no local tree at all. Its far
/// shell's prompt mark is the entire answer, and an empty `procs` beside
/// it must not read as either "free" or "busy".
#[test]
fn wait_free_reads_a_routed_panes_prompt_mark() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    backend.procs_reply = routed_procs(Some(true));

    let json = json_of(run_cli(
        &["lyrel", "wait", "%3", "--until", "free"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "free");
    assert_eq!(json["matched"], true);
}

/// The other half of #840: when the far shell sends no marks there is
/// nothing on this machine that can tell an idle remote prompt from a
/// running remote command. Saying so — and saying it without a `--timeout`
/// to hide behind — beats polling until the caller gives up.
#[test]
fn wait_free_refuses_to_guess_on_a_remote_pane_with_no_integration() {
    let mut backend = mock();
    for _ in 0..4 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    backend.procs_reply = ssh_procs(Some(false), false);

    // Deliberately no `--timeout`: the old code would have polled here
    // until something killed it.
    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "free", "--interval", "50"]),
        &Context::default(),
        &mut backend,
    )
    .expect("an undeterminable pane is an exit code, not an error");
    let Outcome::Exit(1, r) = out else {
        panic!("expected exit 1 with a reason, got {out:?}");
    };
    assert_eq!(r.json["status"], "unknown");
    assert_eq!(r.json["matched"], false);
    let why = r.json["free_unknown"].as_str().expect("a recorded reason");
    assert!(
        why.contains("build-box") && why.contains("shell integration"),
        "the reason names the host and the thing that is missing: {why}"
    );
    assert_eq!(
        backend.procs_calls.len(),
        2,
        "one poll of grace for a handshake in flight, then it stops"
    );
}

/// A structurally undeterminable `free` must not cancel a wait that has
/// another state still able to answer — the agent ladder is read from a
/// different source and knows nothing about the far shell.
#[test]
fn wait_unknown_free_does_not_cancel_a_wait_on_an_agent_state() {
    use lyrel_core::core::cli_agent::AgentStatus;
    let mut backend = mock();
    for _ in 0..3 {
        backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    }
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state(
            3,
            AgentStatus::Done,
        )]));
    backend.procs_reply = routed_procs(None);

    let json = json_of(run_cli(
        &[
            "lyrel",
            "wait",
            "%3",
            "--until",
            "done,free",
            "--interval",
            "50",
        ],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(
        json["status"], "done",
        "the ladder answered while `free` could not"
    );
}

/// The insult on top of the injury: the timeout hint used to send a caller
/// to `--until free` when `--until free` was what had just timed out.
#[test]
fn wait_timeout_does_not_recommend_the_flag_that_just_failed() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    backend.procs_reply = ssh_procs(Some(false), false);

    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "free", "--timeout", "0"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    let Outcome::Exit(124, r) = out else {
        panic!("expected exit 124, got {out:?}");
    };
    assert!(
        !r.human.contains("--until free"),
        "it must not recommend the flag it was given: {}",
        r.human
    );
    assert!(
        r.human
            .contains("could not determine whether this pane is free"),
        "it must say what it could not determine: {}",
        r.human
    );
    assert_eq!(
        r.json["free_unknown"]
            .as_str()
            .map(|s| s.contains("build-box")),
        Some(true),
        "the JSON carries the reason too"
    );
}

/// `no-agent` still points at `--until free` for the caller who has not
/// tried it — that hint is the right one, it was only wrong when repeated
/// back at someone who already used it.
#[test]
fn wait_timeout_still_points_an_agentless_wait_at_free() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));

    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "done", "--timeout", "0"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    let Outcome::Exit(124, r) = out else {
        panic!("expected exit 124, got {out:?}");
    };
    assert!(r.human.contains("--until free"), "{}", r.human);
}

/// A prompt mark outranks a deeper process on a local pane too: whatever
/// drew that prompt is a shell, not work. This is the shape a plain `ssh`
/// takes on a platform where the daemon cannot name the pane as remote —
/// Windows has no foreground process group to read the invocation from.
#[test]
fn wait_free_lets_a_prompt_mark_outrank_a_deeper_process() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::AgentStates(Vec::new()));
    let mut procs = busy_procs();
    procs.context = Some(local_context(Some(true)));
    backend.procs_reply = procs;

    let json = json_of(run_cli(
        &["lyrel", "wait", "%3", "--until", "free"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(json["status"], "free");
}

/// Watching `free` must not cost anything for callers who did not ask:
/// the process tree is a second round trip per poll on top of the agent
/// snapshot, and the default wait is for agents.
#[test]
fn wait_only_reads_the_process_tree_when_free_is_asked_for() {
    use lyrel_core::core::cli_agent::AgentStatus;
    let mut backend = mock();
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state(
            3,
            AgentStatus::Waiting,
        )]));
    run_cli(&["lyrel", "wait", "%3"], &Context::default(), &mut backend);
    assert!(
        backend.procs_calls.is_empty(),
        "the default until-set names no pane-level state"
    );
}

/// A `--timeout` that runs out exits 124 — the `timeout(1)` convention —
/// so scripts can branch on "not yet" separately from "broken".
#[test]
fn wait_timeout_exits_124() {
    use lyrel_core::core::cli_agent::AgentStatus;
    let mut backend = mock();
    backend
        .replies
        .push_back(ReplyOk::AgentStates(vec![agent_state(
            3,
            AgentStatus::Working,
        )]));
    let out = execute(
        cli(&["lyrel", "wait", "%3", "--until", "done", "--timeout", "0"]),
        &Context::default(),
        &mut backend,
    )
    .expect("a timeout is an exit code, not an error");
    match out {
        Outcome::Exit(124, r) => assert_eq!(r.json["timed_out"], true),
        other => panic!("expected exit 124, got {other:?}"),
    }
}
