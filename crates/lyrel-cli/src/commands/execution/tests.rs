use super::EXIT_CODE_UNKNOWN;
use crate::address::Context;
use crate::backend::RunSpec;
use crate::backend::mock::MockBackend;
use crate::commands::Outcome;
use crate::commands::Report;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::mock;
use crate::commands::test_support::segment;
use crate::exec::ExecEnd;
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::daemon::control::ControlRequest;
use std::time::Duration;

#[test]
fn run_passes_the_command_and_its_exit_code_through() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].id;
    let ctx = Context {
        ws: Some(api.to_string()),
        ..Context::default()
    };
    let out = execute(
        cli(&["lyrel", "run", "--keep", "--", "cargo", "test"]),
        &ctx,
        &mut backend,
    )
    .unwrap();
    assert_eq!(
        backend.runs,
        vec![RunSpec {
            workspace: Some(api),
            cwd: None,
            command: vec!["cargo".into(), "test".into()],
            keep: true,
        }]
    );
    assert!(
        matches!(out, Outcome::Exit(0, _)),
        "run's outcome is the child's exit code"
    );
}

#[test]
fn run_keep_spawns_first_then_files_the_pane_into_the_workspace() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].id;
    let out = execute(
        cli(&[
            "lyrel", "run", "--keep", "--ws", "api", "--cwd", "C:\\proj", "--", "cargo", "watch",
        ]),
        &Context::default(),
        &mut backend,
    )
    .unwrap();
    assert_eq!(
        backend.control_calls,
        vec![
            ControlRequest::MachineGet,
            ControlRequest::TabCreate {
                workspace: api,
                at: None,
                pane: PaneSeed {
                    pane: 6,
                    cwd: Some("C:\\proj".into()),
                    ssh_spec: None,
                    agent: None,
                    shell: None,
                },
                tab: None,
            },
        ],
        "the daemon-assigned pane id (6) lands in the tree op, so the spawn came first"
    );
    assert_eq!(
        backend.runs,
        vec![RunSpec {
            workspace: Some(api),
            cwd: Some("C:\\proj".into()),
            command: vec!["cargo".into(), "watch".into()],
            keep: true,
        }]
    );
    assert!(matches!(out, Outcome::Exit(0, _)));
}

#[test]
fn run_without_keep_files_nothing() {
    let mut backend = mock();
    let out = execute(
        cli(&["lyrel", "run", "--", "cargo", "test"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap();
    assert!(
        backend.control_calls.is_empty(),
        "a reaped pane must not be filed into the tree"
    );
    assert_eq!(backend.runs.len(), 1);
    assert!(matches!(out, Outcome::Exit(0, _)));
}

#[test]
fn run_keep_without_a_workspace_names_the_fix() {
    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "run", "--keep", "--", "make"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("a kept pane with no workspace would be an unlisted orphan");
    assert!(err.to_string().contains("--ws"), "{err}");
    assert!(backend.runs.is_empty(), "nothing must be spawned");
}

#[test]
fn a_missing_exit_code_still_exits_nonzero_via_the_note_path() {
    let mut backend = mock();
    backend.run_exit = None;
    let out = execute(
        cli(&["lyrel", "run", "--", "make"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap();
    assert!(
        matches!(out, Outcome::Exit(1, _)),
        "an unknown exit code is still a failure"
    );
    assert!(EXIT_CODE_UNKNOWN.contains("could not be determined"));

    let Outcome::Exit(_, report) = out else {
        unreachable!("just matched")
    };
    assert_eq!(
        report.json["exit_code_known"],
        serde_json::json!(false),
        "--json has to distinguish a stand-in 1 from the command's own 1: {}",
        report.json
    );
}

#[test]
fn run_answers_json_even_though_it_carries_an_exit_code() {
    let mut backend = mock();
    let api = backend.machine.workspaces[0].id;
    let ctx = Context {
        ws: Some(api.to_string()),
        ..Context::default()
    };
    let out = execute(
        cli(&["lyrel", "run", "--json", "--", "cargo", "test"]),
        &ctx,
        &mut backend,
    )
    .unwrap();
    let Outcome::Exit(code, report) = out else {
        panic!("run stands in for its child, so it exits with the child's code");
    };
    assert_eq!(code, 0);
    assert_eq!(report.json["exit"], serde_json::json!(0), "{}", report.json);
    assert_eq!(
        report.json["exit_code_known"],
        serde_json::json!(true),
        "{}",
        report.json
    );
    assert!(
        report.json["pane"].as_u64().is_some(),
        "the pane that ran it is part of the answer: {}",
        report.json
    );
    assert!(
        report.human.is_empty(),
        "the command's own output already streamed; the report must not repeat it"
    );
}

fn exec_run(end: ExecEnd, output: &[u8]) -> crate::exec::ExecRun {
    crate::exec::ExecRun {
        end,
        output: output.to_vec(),
        dropped: 0,
        size: segment(b"").size,
    }
}

fn exec_outcome(args: &[&str], ctx: &Context, backend: &mut MockBackend) -> (i32, Report) {
    match execute(cli(args), ctx, backend) {
        Ok(Outcome::Exit(code, report)) => (code, report),
        other => panic!("exec stands in for the command, so it exits: {other:?}"),
    }
}

#[test]
fn exec_types_one_line_and_passes_the_exit_code_through() {
    let mut backend = mock();
    backend.exec_run = exec_run(
        ExecEnd::Finished {
            exit: Some(3),
            ran: true,
        },
        b"built\r\n",
    );
    let (code, report) = exec_outcome(
        &[
            "lyrel",
            "exec",
            "%1",
            "--raw",
            "--timeout",
            "30",
            "--",
            "make",
            "&&",
            "ls",
        ],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(code, 3, "the command's own code, not ours");
    assert_eq!(
        backend.execs,
        vec![(1, b"make && ls\r".to_vec(), Some(Duration::from_secs(30)))],
        "the words are one shell line, typed and submitted"
    );
    assert_eq!(report.human, "built\r\n");
    assert_eq!(report.json["exit"], 3);
    assert_eq!(report.json["exit_code_known"], true);
    assert_eq!(report.json["command"], "make && ls");
    assert_eq!(report.json["output"], "built\r\n");
    assert_eq!(report.json["timed_out"], false);
    // Nothing was sent anywhere but through the exec seam.
    assert!(backend.sent.is_empty());
}

#[test]
fn exec_renders_the_output_the_way_capture_plain_does() {
    let mut backend = mock();
    backend.exec_run = exec_run(
        ExecEnd::Finished {
            exit: Some(0),
            ran: true,
        },
        b"\x1b[31mred\x1b[0m\r\nprogress 10%\rprogress 100%\r\n",
    );
    let (code, report) = exec_outcome(
        &["lyrel", "exec", "%1", "--", "x"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(code, 0);
    assert!(!report.human.contains('\u{1b}'), "{:?}", report.human);
    assert!(report.human.contains("red"), "{:?}", report.human);
    assert!(report.human.contains("progress 100%"), "{:?}", report.human);
    assert!(
        !report.human.contains("progress 10%\n"),
        "{:?}",
        report.human
    );
}

#[test]
fn exec_refuses_a_pane_it_cannot_follow_before_typing_anything() {
    // The backend sends nothing on these; the verb has to turn them into
    // an error that says what to do instead of an exit code that says
    // nothing.
    let mut backend = mock();
    backend.exec_run = exec_run(ExecEnd::NoIntegration, b"");
    let err = execute(
        cli(&["lyrel", "exec", "%1", "--", "ls"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("shell-integration"), "{err}");
    assert!(err.contains("wait --until free"), "{err}");

    let mut backend = mock();
    backend.exec_run = exec_run(ExecEnd::Busy, b"");
    let err = execute(
        cli(&["lyrel", "exec", "%1", "--", "ls"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not at a shell prompt"), "{err}");

    // The caller's own pane can only ever be busy — running this.
    let ctx = Context {
        pane: Some("%1".into()),
        ..Context::default()
    };
    let err = execute(cli(&["lyrel", "exec", "--", "ls"]), &ctx, &mut backend)
        .unwrap_err()
        .to_string();
    assert!(err.contains("this shell"), "{err}");
}

#[test]
fn exec_refuses_a_line_the_editor_would_act_on() {
    for line in ["make\nrm -rf build", "ls\tx", "a\u{1b}b"] {
        let mut backend = mock();
        let err = execute(
            cli(&["lyrel", "exec", "%1", "--", line]),
            &Context::default(),
            &mut backend,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("control"), "{line:?}: {err}");
        assert!(backend.execs.is_empty(), "nothing may be typed: {line:?}");
    }
}

#[test]
fn exec_reports_every_way_it_can_end_without_an_exit_code() {
    let cases = [
        (ExecEnd::TimedOut { started: true }, 124, true, false),
        (ExecEnd::TimedOut { started: false }, 124, true, false),
        (ExecEnd::PaneExited(Some(0)), 1, false, true),
        (
            ExecEnd::Finished {
                exit: None,
                ran: true,
            },
            1,
            false,
            false,
        ),
        (
            ExecEnd::Finished {
                exit: None,
                ran: false,
            },
            1,
            false,
            false,
        ),
    ];
    for (end, want, timed_out, pane_exited) in cases {
        let mut backend = mock();
        backend.exec_run = exec_run(end.clone(), b"partial\r\n");
        let (code, report) = exec_outcome(
            &["lyrel", "exec", "%1", "--raw", "--timeout", "1", "--", "x"],
            &Context::default(),
            &mut backend,
        );
        assert_eq!(code, want, "{end:?}");
        assert_eq!(report.json["exit_code_known"], false, "{end:?}");
        assert_eq!(report.json["timed_out"], timed_out, "{end:?}");
        assert_eq!(report.json["pane_exited"], pane_exited, "{end:?}");
        assert_eq!(
            report.human, "partial\r\n",
            "what did arrive is still the answer: {end:?}"
        );
    }
}
