use super::*;

pub(super) fn run_streams_output_and_passes_the_exit_code(daemon: &Daemon) {
    let echo = one_shot("echo lyrel_e2e_run_marker");
    let mut args: Vec<&str> = vec!["run", "--"];
    args.extend(echo.iter().map(String::as_str));
    let out = daemon.run(&args);
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lyrel_e2e_run_marker"), "{stdout}");

    let exit = one_shot("exit 7");
    let mut args: Vec<&str> = vec!["run", "--"];
    args.extend(exit.iter().map(String::as_str));
    let out = daemon.run(&args);
    assert_eq!(
        out.status.code(),
        Some(7),
        "the child's exit code must pass through: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub(super) fn run_keep_files_the_pane_so_ls_shows_it(daemon: &Daemon) {
    let ws = daemon.run_json(&["ws", "new", "runws"]);
    let ws_id = ws["id"].as_str().expect("ws new prints the id").to_string();

    let mut args: Vec<String> = vec![
        "run".into(),
        "--keep".into(),
        "--ws".into(),
        ws_id.clone(),
        "--".into(),
    ];
    args.extend(one_shot("echo lyrel_e2e_keep_marker"));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = daemon.run(&arg_refs);
    assert!(
        out.status.success(),
        "run --keep failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let listed = daemon.run_json(&["ls"]);
    let workspaces = listed["workspaces"]
        .as_array()
        .expect("ls --json lists workspaces");
    let ours = workspaces
        .iter()
        .find(|w| w["id"].as_str() == Some(ws_id.as_str()))
        .unwrap_or_else(|| panic!("the target workspace is missing from ls: {listed}"));
    assert_eq!(
        ours["panes"].as_u64(),
        Some(1),
        "the kept pane must be filed where every listing sees it: {listed}"
    );

    let panes = daemon.run_json(&["pane", "ls", &ws_id]);
    let filed = panes["panes"]
        .as_array()
        .expect("pane ls --json lists panes");
    assert_eq!(filed.len(), 1, "{panes}");
    assert!(filed[0]["pane"].as_u64().is_some_and(|p| p >= 1), "{panes}");
}

/// `lyrel … | head -1` must end the way `cat … | head -1` ends. Rust ignores
/// SIGPIPE and `println!` panics on the resulting error, so without the fix
/// this printed a panic and a backtrace note on a correct invocation.
///
/// Both write paths are covered: `ls` goes through the report emitter, `run`
/// through the loop that streams a child's output. The reader is dropped
/// immediately, long before either has anything to say, so the very first
/// write lands on a pipe with no other end — no need to guess at a buffer size.
pub(super) fn a_reader_that_hung_up_ends_the_pipeline_quietly(daemon: &Daemon) {
    daemon.run_ok(&["ws", "new", "pipews"]);

    let printer = one_shot("echo lyrel_e2e_pipe_marker");
    let mut streaming: Vec<&str> = vec!["run", "--"];
    streaming.extend(printer.iter().map(String::as_str));

    for args in [vec!["ls"], streaming] {
        let mut child = daemon
            .cli(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("could not spawn lyrel {args:?}: {e}"));
        drop(child.stdout.take().expect("stdout was piped"));

        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("stderr was piped")
            .read_to_string(&mut stderr)
            .expect("reading lyrel's stderr");
        let status = child.wait().expect("waiting for lyrel");

        assert!(
            !stderr.contains("panicked"),
            "lyrel {args:?} panicked when its reader hung up: {stderr}"
        );
        assert!(
            !stderr.to_lowercase().contains("broken pipe"),
            "a hung-up reader is how a pipeline ends, not something to report: \
             lyrel {args:?} said {stderr}"
        );
        // Unix dies of SIGPIPE, so there is no code at all; Windows exits 0.
        // Either way it must not be the failure exit, which is what the error
        // path used to produce.
        assert_ne!(
            status.code(),
            Some(1),
            "lyrel {args:?} treated a hung-up reader as a failure: {stderr}"
        );
    }
}

/// Block until the pane's shell has drawn its first prompt: before that there
/// are no marks for `exec` to follow, and it refuses rather than guesses.
pub(super) fn await_prompt(daemon: &Daemon, address: &str) {
    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let procs = daemon.run_json(&["procs", address]);
        if procs["context"]["at_prompt"].as_bool() == Some(true) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never reported a prompt; last procs: {procs}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub(super) fn exec_returns_the_commands_output_and_exit_code(daemon: &Daemon) {
    // The marks come from lyrel's integration for zsh/bash/fish/nushell/pwsh;
    // the shell a Windows runner defaults to is not one this can count on.
    if cfg!(windows) {
        return;
    }
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    let address = format!("%{pane}");
    await_prompt(daemon, &address);

    // `sh -c` so the exit code does not depend on which shell the pane runs.
    let out = daemon.run(&[
        "exec",
        &address,
        "--timeout",
        "60",
        "--",
        "sh",
        "-c",
        "'echo lyrel_e2e_exec_$((6*7)); exit 3'",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(3),
        "exec exits with the command's code; stdout {stdout:?}, stderr {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("lyrel_e2e_exec_42"), "{stdout:?}");
    assert!(
        !stdout.contains("6*7") && !stdout.contains("exit 3"),
        "the typed line is not the command's output: {stdout:?}"
    );

    // The pane is back at its prompt, so it takes the next one straight away.
    // `--json` ahead of `--`: after it, it would be an argument to `true`.
    let out = daemon.run_ok(&["exec", &address, "--json", "--timeout", "60", "--", "true"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("--json prints JSON");
    assert_eq!(json["exit"], 0, "{json}");
    assert_eq!(json["exit_code_known"], true, "{json}");
    assert_eq!(json["pane"], pane, "{json}");
    assert_eq!(
        json["output"], "",
        "a command that prints nothing has nothing to show — not the shell's \
         prompt preparation: {json}"
    );
}

pub(super) fn exec_refuses_a_pane_with_no_prompt_marks(daemon: &Daemon) {
    if cfg!(windows) {
        return;
    }
    let pane = bare_pane(daemon, "/bin/cat");
    let out = daemon.run(&["exec", &format!("%{pane}"), "--", "echo", "never"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("shell-integration"), "{stderr}");
    // Refused before typing: nothing reached the pane.
    let seen = daemon.run_ok(&["capture", &format!("%{pane}"), "--scrollback"]);
    assert!(!seen.contains("never"), "{seen:?}");
}
