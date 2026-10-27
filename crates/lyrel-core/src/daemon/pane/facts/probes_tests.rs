use super::*;
use super::super::*;
use super::super::tests::{test_state, ws, null_writer};
use super::output_marks::{OscSniffer};
use super::probes::is_foreground_command;

/// The client forgets its cwd on every `RemoteContext`, so a replay that
/// sent the far shell's directory first would have it wiped straight away.
#[test]
fn a_window_reattaching_to_an_ssh_pane_keeps_its_remote_directory() {
    let mut st = test_state(true);
    st.facts.remote = Some(RemoteContext {
        kind: RemoteKind::NativeSsh,
        argv: Vec::new(),
        target: "alice@box".into(),
    });
    st.facts.cwd = Some(PathBuf::from("/home/alice/my_service"));
    let (tx, rx) = std::sync::mpsc::channel();
    replay_state(&st, &tx, false);
    drop(tx);
    let order: Vec<&str> = rx
        .iter()
        .filter_map(|m| match m {
            DaemonMsg::RemoteContext(_) => Some("remote"),
            DaemonMsg::Cwd(_) => Some("cwd"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["remote", "cwd"]);
}

#[test]
fn foreground_command_distinguishes_the_shell_from_a_command() {
    assert!(!is_foreground_command(Some(1000), Some(1000)));
    assert!(is_foreground_command(Some(2000), Some(1000)));
    assert!(!is_foreground_command(None, Some(1000)));
    assert!(!is_foreground_command(Some(2000), None));
    assert!(!is_foreground_command(Some(0), Some(1000)));
}

#[test]
fn foreground_program_prompt_marks_do_not_claim_the_prompt() {
    let mut s = OscSniffer::new();
    let mut signals = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    assert!(
        signals.shell.last().unwrap().at_prompt,
        "the raw marks read as at-prompt"
    );

    let ssh_running = is_foreground_command(Some(2000), Some(1000));
    if signals.shell.iter().any(|st| st.at_prompt) && ssh_running {
        suppress_relayed_prompt_marks(&mut signals.shell);
    }
    assert!(
        !signals.shell.last().unwrap().at_prompt,
        "a foreground program's prompt marks must not engage the local editor"
    );
    assert!(
        signals.shell.last().unwrap().mark_at_prompt,
        "the mark's own reading survives: over `ssh` it is the far shell speaking, and \
         the only thing this side knows about whether it is busy (#840)"
    );

    let mut local = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    let shell_idle = is_foreground_command(Some(1000), Some(1000));
    if local.shell.iter().any(|st| st.at_prompt) && shell_idle {
        suppress_relayed_prompt_marks(&mut local.shell);
    }
    assert!(local.shell.last().unwrap().at_prompt);
}

/// A prompt mark that arrives while the pane is pointed at a remote host
/// can only be the far shell's — the near one cannot be at a prompt while
/// the connection owns its pty. That is what proves the far side runs the
/// shell integration, and it is the difference between "the remote is
/// busy" and "we cannot tell" (#840).
#[test]
fn a_remote_panes_prompt_mark_latches_and_a_new_hop_clears_it() {
    let mark = |mark_at_prompt: bool, command: Option<&str>| ShellState {
        active: true,
        at_prompt: false,
        mark_at_prompt,
        last_exit_code: None,
        command: command.map(str::to_string),
    };
    let hop = |target: &str| RemoteContext {
        kind: RemoteKind::Ssh,
        argv: vec!["ssh".into(), target.into()],
        target: target.into(),
    };
    /// One pass of the reader's ordering: signals, then the hop the same
    /// read detected, then the latch.
    fn read(st: &mut PaneState, shell: Vec<ShellState>, remote: Option<Option<RemoteContext>>) {
        let saw_prompt_mark = shell.iter().any(|s| s.mark_at_prompt);
        (st).with_facts(|facts, emit| apply_signals(facts, SniffSignals {
            shell,
            ..SniffSignals::default()
        }, emit));
        if let Some(remote) = remote {
            (st).with_facts(|facts, emit| apply_remote_context(facts, remote, emit));
        }
        latch_remote_prompt(&mut (st).facts, saw_prompt_mark);
    }

    // Local: the same mark proves nothing about any far side.
    let mut st = test_state(true);
    read(&mut st, vec![mark(true, None)], None);
    assert!(!st.facts.remote_prompt_seen);

    // The near shell's own "I started `ssh`" — no prompt in it — must not
    // latch, even though the hop lands in the same read.
    read(
        &mut st,
        vec![mark(false, Some("ssh build-box"))],
        Some(Some(hop("build-box"))),
    );
    assert!(
        !st.facts.remote_prompt_seen,
        "starting the connection is not evidence about what is behind it"
    );

    // The far shell's opening prompt does — including when it arrives in
    // the very read that first sees the pane as remote, which on a unix
    // host is the common case: the relayed prompt is suppressed, so it
    // cannot trigger the immediate re-probe that would have split the two.
    let mut fresh = test_state(true);
    read(
        &mut fresh,
        vec![mark(true, None)],
        Some(Some(hop("build-box"))),
    );
    assert!(fresh.facts.remote_prompt_seen);
    read(&mut st, vec![mark(true, None)], None);
    assert!(st.facts.remote_prompt_seen);

    // A second hop has to prove itself over again.
    read(&mut st, Vec::new(), Some(Some(hop("other-box"))));
    assert!(!st.facts.remote_prompt_seen);

    // And coming back to the local shell clears it too.
    read(&mut st, vec![mark(true, None)], None);
    assert!(st.facts.remote_prompt_seen);
    read(&mut st, Vec::new(), Some(None));
    assert!(!st.facts.remote_prompt_seen);
}

#[test]
fn a_shutdown_after_the_foreground_exit_does_not_reclaim_the_pane() {
    use crate::core::cli_agent::{CLIAgent, parse_agent_event_body};

    let mut state = test_state(true);
    (&mut state).with_facts(|facts, emit| apply_agent(facts, Some((CLIAgent::OhMyPi, vec!["omp".into()])), emit));
    (&mut state).with_facts(|facts, emit| apply_agent(facts, None, emit));
    let ended = parse_agent_event_body(
        br#"{"agent":"omp","event":"session-end","session_id":"already-exited"}"#,
    )
    .unwrap();
    (&mut state).with_facts(|facts, emit| apply_agent_signals(facts, vec![ended], None, emit));
    assert!(state.facts.agent.is_none());
    assert!(state.facts.agent_session.is_none());
    assert!(observed_facts(&state.facts).agent.is_none());
}

#[test]
fn probed_cwd_corrects_a_pane_whose_shell_never_reports_osc7() {
    let mut st = test_state(true);
    st.facts.cwd = Some(PathBuf::from("/Users/alice"));
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(Arc::new(tx));

    (&mut st).with_facts(|facts, emit| apply_probed_cwd(facts, Some(PathBuf::from("/Users/alice/dev/lyrel")), emit));

    assert_eq!(st.facts.cwd.as_deref(), Some(Path::new("/Users/alice/dev/lyrel")));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/dev/lyrel"))
    );
}

#[cfg(unix)]
#[test]
fn probed_cwd_keeps_the_shells_spelling_for_a_symlinked_path() {
    let tmp = std::env::temp_dir().join(format!("lyrel-cwd-{}", std::process::id()));
    let real = tmp.join("real");
    let link = tmp.join("link");
    std::fs::create_dir_all(&real).unwrap();
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let mut st = test_state(true);
    st.facts.cwd = Some(link.clone());
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(Arc::new(tx));

    (&mut st).with_facts(|facts, emit| apply_probed_cwd(facts, Some(real.canonicalize().unwrap()), emit));

    assert_eq!(st.facts.cwd.as_deref(), Some(link.as_path()));
    assert!(rx.try_recv().is_err(), "same directory → nothing to tell");

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn probed_cwd_matching_the_reported_one_says_nothing() {
    let mut st = test_state(true);
    st.facts.cwd = Some(PathBuf::from("/Users/alice/dev/lyrel"));
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(Arc::new(tx));

    (&mut st).with_facts(|facts, emit| apply_probed_cwd(facts, Some(PathBuf::from("/Users/alice/dev/lyrel")), emit));

    assert_eq!(st.facts.cwd.as_deref(), Some(Path::new("/Users/alice/dev/lyrel")));
    assert!(rx.try_recv().is_err());
}

#[test]
fn probed_cwd_declines_to_speak_for_a_remote_pane() {
    let mut st = test_state(true);
    st.facts.remote = Some(RemoteContext {
        kind: RemoteKind::Ssh,
        argv: vec!["ssh".into(), "build-box".into()],
        target: "build-box".into(),
    });
    st.facts.cwd = None;
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(Arc::new(tx));

    (&mut st).with_facts(|facts, emit| apply_probed_cwd(facts, Some(PathBuf::from("/Users/alice/dev/lyrel")), emit));

    assert_eq!(st.facts.cwd, None);
    assert!(rx.try_recv().is_err());
}

#[test]
fn probed_cwd_absent_leaves_the_reported_cwd_alone() {
    let mut st = test_state(true);
    st.facts.cwd = Some(PathBuf::from("/work"));
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(Arc::new(tx));

    (&mut st).with_facts(|facts, emit| apply_probed_cwd(facts, None, emit));

    assert_eq!(st.facts.cwd.as_deref(), Some(Path::new("/work")));
    assert!(rx.try_recv().is_err());
}

/// The foreground probe only ever runs on output, and the prompt a shell
/// draws after a command is the last output a pane produces until the user
/// types again. So an `ssh` that exited inside the poll interval used to
/// leave the pane reporting itself as remote indefinitely: nothing came
/// along to probe on. A prompt mark now forces the probe (#817).
#[test]
fn a_prompt_mark_reprobes_the_foreground_inside_the_poll_interval() {
    /// Hands the reader one chunk per `read`, so two prompt marks arrive as
    /// two passes through the loop a few microseconds apart — well inside
    /// `REMOTE_CONTEXT_POLL_INTERVAL`.
    struct Chunks(std::collections::VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                Some(chunk) => {
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    Ok(chunk.len())
                }
                None => Ok(0),
            }
        }
    }

    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(Arc::new(sub_tx));

    let probes_taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let taken = probes_taken.clone();
    let remote = Box::new(move || {
        // First probe: `ssh` holds the pty. Every one after: it is gone.
        (taken.fetch_add(1, Ordering::SeqCst) == 0).then(|| RemoteContext {
            kind: RemoteKind::Ssh,
            argv: vec!["ssh".into(), "box".into()],
            target: "box".into(),
        })
    });

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(Chunks(
            [
                b"\x1b]133;C;ssh box\x07".to_vec(),
                b"\x1b]133;D;0\x07".to_vec(),
            ]
            .into_iter()
            .collect(),
        )),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote,
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    handle.join().unwrap();

    assert_eq!(
        probes_taken.load(Ordering::SeqCst),
        2,
        "the prompt mark did not force a second probe"
    );
    assert!(
        state.lock().unwrap().facts.remote.is_none(),
        "the pane still reports the ssh session it has already left"
    );
    let reported: Vec<Option<String>> = sub_rx
        .try_iter()
        .filter_map(|msg| match msg {
            DaemonMsg::RemoteContext(ctx) => Some(ctx.map(|c| c.target)),
            _ => None,
        })
        .collect();
    assert_eq!(
        reported,
        vec![Some("box".to_string()), None],
        "the client was never told the pane came home"
    );
}

#[test]
#[cfg(unix)]
fn reader_gates_indirect_media_before_first_ssh_bytes_and_restores_local_queries() {
    use base64::Engine as _;
    use std::sync::atomic::AtomicUsize;
    struct Chunks {
        chunks: std::collections::VecDeque<(usize, Vec<u8>)>,
        phase: Arc<AtomicUsize>,
    }
    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some((phase, chunk)) = self.chunks.pop_front() else {
                return Ok(0);
            };
            self.phase.store(phase, Ordering::SeqCst);
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }
    struct Replies(Arc<Mutex<Vec<u8>>>);
    impl Write for Replies {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tty-graphics-protocol-host-only.rgba");
    std::fs::write(&path, [255, 0, 0, 255]).unwrap();
    let name =
        base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_encoded_bytes());
    let query = |id| format!("\x1b_Ga=q,i={id},t=f,f=32,s=1,v=1;{name}\x1b\\").into_bytes();
    let run = |chunks: Vec<(usize, Vec<u8>)>, same_group: bool, unknown: bool| {
        let phase = Arc::new(AtomicUsize::new(0));
        let group_phase = phase.clone();
        let remote_phase = phase.clone();
        let sink = Arc::new(Mutex::new(Vec::new()));
        DaemonPane::spawn_reader(
            Arc::new(Mutex::new(test_state(true))),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(Chunks {
                chunks: chunks
                    .into_iter()
                    .filter(|(_, bytes)| !bytes.is_empty())
                    .collect(),
                phase,
            }),
            Arc::new(Mutex::new(Box::new(Replies(sink.clone())))),
            || false,
            ForegroundProbes {
                group: Box::new(move || {
                    let remote = matches!(group_phase.load(Ordering::SeqCst), 1 | 2);
                    if unknown && remote {
                        None
                    } else {
                        Some(if remote && !same_group { 11 } else { 10 })
                    }
                }),
                remote: Box::new(move || {
                    (!unknown && matches!(remote_phase.load(Ordering::SeqCst), 1 | 2)).then(
                        || RemoteContext {
                            kind: RemoteKind::Ssh,
                            argv: vec!["ssh".into(), "other-host".into()],
                            target: "other-host".into(),
                        },
                    )
                }),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();
        let replies = sink.lock().unwrap().clone();
        replies
    };
    let remote_query = query(2);
    for same_group in [false, true] {
        for cut in 0..=remote_query.len() {
            let replies = run(
                vec![
                    (0, query(1)),
                    (1, remote_query[..cut].to_vec()),
                    (2, remote_query[cut..].to_vec()),
                    (2, query(3)),
                    (3, query(4)),
                ],
                same_group,
                false,
            );
            assert_eq!(replies, b"\x1b_Gi=1;OK\x1b\\\x1b_Gi=2;ENOTSUPPORTED:Host-local transfer unavailable\x1b\\\x1b_Gi=3;ENOTSUPPORTED:Host-local transfer unavailable\x1b\\\x1b_Gi=4;OK\x1b\\", "same_group={same_group}, split={cut}");
        }
    }
    let split = (name.len() / 8) * 4;
    for (same_group, unknown) in [(false, false), (true, false), (false, true)] {
        let replies = run(
            vec![
                (
                    0,
                    format!(
                        "\x1b_Ga=q,i=9,t=f,f=32,s=1,v=1,m=1;{}\x1b\\",
                        &name[..split]
                    )
                    .into_bytes(),
                ),
                (1, b"\x1b_Gm=0;".to_vec()),
                (3, format!("{}\x1b\\", &name[split..]).into_bytes()),
                (3, query(4)),
            ],
            same_group,
            unknown,
        );
        assert_eq!(
            replies,
            b"\x1b_Gi=9;ENOTSUPPORTED:Host-local transfer unavailable\x1b\\\x1b_Gi=4;OK\x1b\\",
            "a pending transfer that crossed remote/unknown cannot become local again: same_group={same_group}, unknown={unknown}"
        );
    }
    assert!(path.exists(), "query does not delete a plain file");
}

#[test]
fn reader_poll_applies_the_probed_cwd() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    {
        let mut st = state.lock().unwrap();
        st.facts.cwd = Some(PathBuf::from("/Users/alice"));
        st.subscriber = Some(Arc::new(sub_tx));
    }

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(b"alice@host ~ % ".to_vec())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| Some(PathBuf::from("/Users/alice/dev/lyrel"))),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    handle.join().unwrap();

    assert_eq!(
        state.lock().unwrap().facts.cwd.as_deref(),
        Some(Path::new("/Users/alice/dev/lyrel"))
    );
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(_))));
    assert!(
        matches!(sub_rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/dev/lyrel"))
    );
}
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn live_pane_reports_an_uninstrumented_shells_cwd() {
        let target = std::path::Path::new("/usr");

        let (tx, rx) = mpsc::channel();
        let pane = DaemonPane::spawn(
            1,
            Some(PathBuf::from("/")),
            ws(80, 24),
            Some(ShellSpec {
                program: "sh".into(),
                args: vec!["-c".into(), "cd /usr && exec cat".into()],
                args_are_lyrel_defaults: false,
            }),
            None,
            None,
            None,
            false,
            || {},
        )
        .expect("spawn pane");
        pane.attach(tx);

        let mut reported = None;
        for _ in 0..200 {
            pane.write_input(b"\n");
            while let Ok(msg) = rx.try_recv() {
                if let DaemonMsg::Cwd(p) = msg {
                    reported = Some(p);
                }
            }
            if reported.as_deref() == Some(target) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pane.kill();

        assert_eq!(
            reported.as_deref(),
            Some(target),
            "a pane whose shell emits no OSC 7 must still report where it \
             actually is — this is what a new tab inherits (issue #187)"
        );
    }
