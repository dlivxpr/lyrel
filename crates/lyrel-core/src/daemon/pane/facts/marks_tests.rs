use super::*;
use super::super::*;
use super::super::test_support::{test_state, drain};
use super::output_marks::{OscSniffer, MAX_OSC_TITLE, parse_osc7, strip_uri_drive_slash, percent_decode, hex_val};

#[test]
fn sniff_osc7_cwd() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]7;file://host/Users/me/dev\x07");
    assert_eq!(sig.cwd, Some(PathBuf::from("/Users/me/dev")));
}

/// The tree's only source of the name a window actually shows on a tab. A
/// tokenizer that does not list 0 and 2 sniffs nothing here, and the
/// switcher goes back to calling every agent tab "Claude Code".
#[test]
fn sniff_osc_title() {
    let mut s = OscSniffer::new();
    assert_eq!(
        s.feed(b"\x1b]0;user@host:~/dev\x07").title.as_deref(),
        Some("user@host:~/dev")
    );
    assert_eq!(
        s.feed(b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\")
            .title
            .as_deref(),
        Some("✳ fixing the switcher")
    );
    // The last one in a read is the one that stuck.
    assert_eq!(
        s.feed(b"\x1b]0;first\x07\x1b]0;second\x07")
            .title
            .as_deref(),
        Some("second")
    );
    // An empty title is a reset, and has to be told apart from a read with
    // no title in it at all.
    assert_eq!(s.feed(b"\x1b]2;\x07").title.as_deref(), Some(""));
    assert_eq!(s.feed(b"plain output\r\n").title, None);
    // OSC 1 names the icon, which the window ignores; so do we.
    assert_eq!(s.feed(b"\x1b]1;icon\x07").title, None);

    let long = format!("\x1b]0;{}\x07", "t".repeat(MAX_OSC_TITLE + 10));
    assert_eq!(
        s.feed(long.as_bytes()).title.map(|t| t.chars().count()),
        Some(MAX_OSC_TITLE)
    );
}

/// Titles arrive interleaved with everything else in the same read, and the
/// tokenizer has one identifier filter for the lot: sniffing a title must
/// not cost a cwd or a prompt mark.
#[test]
fn a_title_does_not_swallow_the_other_marks_in_the_same_read() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]0;user@host:~/dev\x07\x1b]7;file://host/dev\x07\x1b]133;A\x07");
    assert_eq!(sig.title.as_deref(), Some("user@host:~/dev"));
    assert_eq!(sig.cwd, Some(PathBuf::from("/dev")));
    assert!(sig.shell.last().unwrap().at_prompt);
}

#[test]
fn a_title_is_kept_until_it_changes_and_a_reset_clears_it() {
    let mut st = test_state(true);
    (&mut st).with_facts(|facts, emit| apply_signals(facts, SniffSignals {
        title: Some("✳ fixing the switcher".into()),
        ..SniffSignals::default()
    }, emit));
    assert_eq!(st.facts.osc_title.as_deref(), Some("✳ fixing the switcher"));

    // A read with no title in it leaves the stored one alone.
    (&mut st).with_facts(|facts, emit| apply_signals(facts, SniffSignals::default(), emit));
    assert_eq!(st.facts.osc_title.as_deref(), Some("✳ fixing the switcher"));

    (&mut st).with_facts(|facts, emit| apply_signals(facts, SniffSignals {
        title: Some(String::new()),
        ..SniffSignals::default()
    }, emit));
    assert_eq!(st.facts.osc_title, None, "an empty title clears, not blanks");
}

/// #889: the tab went on reading "✳ fixing the switcher" long after Claude
/// Code had exited and the pane was back at its own prompt, because
/// nothing in the pane path ever retired an OSC 0/2 — only another one
/// replaced it. The `D` mark says the command that wrote it is over.
#[test]
fn a_title_a_command_set_is_retired_when_that_command_finishes() {
    let mut st = test_state(true);
    let mut s = OscSniffer::new();

    let mut read = |st: &mut PaneState, bytes: &[u8]| (st).with_facts(|facts, emit| apply_signals(facts, s.feed(bytes), emit));

    read(
        &mut st,
        b"\x1b]7;file://h/work/lyrel\x07\x1b]133;A\x07\x1b]133;B\x07",
    );
    read(&mut st, b"\x1b]133;C;claude\x07");
    read(&mut st, b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\");
    assert_eq!(
        st.facts.osc_title.as_deref(),
        Some("✳ fixing the switcher"),
        "a running command names the tab"
    );

    // The user's precmd chain can take hundreds of ms, which is why the
    // `D` emitter is prepended to it — so the mark routinely lands in a
    // read of its own, ahead of anything the next prompt writes.
    read(&mut st, b"\x1b]133;D;0\x07");
    assert_eq!(
        st.facts.osc_title, None,
        "the program that wrote the title has exited"
    );
    assert_eq!(
        st.facts.cwd.as_deref(),
        Some(std::path::Path::new("/work/lyrel")),
        "and the rung below it in the label ladder still knows the place"
    );
}

/// The two cases the retirement must not touch: a program that is still
/// running, and a shell that titles its own prompt.
#[test]
fn a_running_program_and_a_shells_own_prompt_title_both_keep_theirs() {
    let mut st = test_state(true);
    let mut s = OscSniffer::new();

    (&mut st).with_facts(|facts, emit| apply_signals(facts, s.feed(b"\x1b]133;C;vim\x07\x1b]2;vim\x1b\\"), emit));
    assert_eq!(
        st.facts.osc_title.as_deref(),
        Some("vim"),
        "nothing said the command ended"
    );

    // A shell that re-titles itself does it between the `D` and the `A`
    // (lyrel's zsh helper prepends the `D`; the PowerShell one titles in
    // its prompt function), so the title is the last word in the read.
    (&mut st).with_facts(|facts, emit| apply_signals(facts, s.feed(b"\x1b]133;D;0\x07\x1b]0;me@box:~/dev\x07\x1b]133;A\x07"), emit));
    assert_eq!(
        st.facts.osc_title.as_deref(),
        Some("me@box:~/dev"),
        "the prompt's own title outranks the retirement it follows"
    );

    // And that prompt title is nobody's command to retire.
    (&mut st).with_facts(|facts, emit| apply_signals(facts, s.feed(b"\x1b]133;C;ls\x07\x1b]133;D;0\x07"), emit));
    assert_eq!(st.facts.osc_title.as_deref(), Some("me@box:~/dev"));
}

/// A pane with no shell integration never sees a mark, so nothing changes
/// for it: whatever it last called itself is all lyrel has.
#[test]
fn a_pane_without_shell_integration_keeps_its_title() {
    let mut st = test_state(true);
    let mut s = OscSniffer::new();
    (&mut st).with_facts(|facts, emit| apply_signals(facts, s.feed(b"\x1b]0;user@host:~/dev\x07"), emit));
    (&mut st).with_facts(|facts, emit| apply_signals(facts, s.feed(b"lots of output\r\n"), emit));
    assert_eq!(st.facts.osc_title.as_deref(), Some("user@host:~/dev"));
}

#[test]
fn sniff_osc133_prompt() {
    let mut s = OscSniffer::new();
    let b = s.feed(b"\x1b]133;B\x07");
    assert!(b.shell.last().unwrap().active);
    assert!(b.shell.last().unwrap().at_prompt);

    let c = s.feed(b"\x1b]133;C\x07");
    assert!(!c.shell.last().unwrap().at_prompt);

    let d = s.feed(b"\x1b]133;D;130\x07");
    assert!(d.shell.last().unwrap().at_prompt);
    assert_eq!(d.shell.last().unwrap().last_exit_code, Some(130));
}

#[test]
fn a_full_command_cycle_in_one_chunk_still_reports_leaving_the_prompt() {
    let mut s = OscSniffer::new();
    s.feed(b"\x1b]133;A\x07\x1b]133;B\x07");

    let sig =
        s.feed(b"\x1b]133;C;echo%20hi\x07hi\r\n\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07");
    let states: Vec<bool> = sig.shell.iter().map(|s| s.at_prompt).collect();
    assert_eq!(
        states,
        vec![false, true],
        "the chunk must report leaving the prompt and coming back, not just the end state"
    );
    assert_eq!(sig.shell.last().unwrap().last_exit_code, Some(0));
    assert_eq!(sig.shell.last().unwrap().command, None);
}

#[test]
fn marks_on_the_same_side_of_the_prompt_boundary_fold_into_one_state() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]133;D;3\x07\x1b]133;A\x07\x1b]133;B\x07");
    assert_eq!(sig.shell.len(), 1, "D/A/B is one at-prompt state");
    assert!(sig.shell[0].at_prompt);
    assert_eq!(sig.shell[0].last_exit_code, Some(3));
}

#[test]
fn sniff_osc133_stray_marks_do_not_reapply_mark_detection() {
    let mut s = OscSniffer::new();

    let mut prev = ShellState::default();
    let c = s
        .feed(b"\x1b]133;C;.%5Cdev.ps1;aid=lyrel\x07")
        .shell
        .pop()
        .unwrap();
    assert!(shell_mark_capture_changed(&prev, &c));
    assert_eq!(
        agent_from_shell_mark(&c, &std::collections::HashMap::new()),
        None
    );
    prev = c;

    let ab = s.feed(b"\x1b]133;A\x1b]133;B\x07").shell.pop().unwrap();
    assert!(!shell_mark_capture_changed(&prev, &ab));
    prev = ab;

    let d = s.feed(b"\x1b]133;D;0;aid=lyrel\x07").shell.pop().unwrap();
    assert!(shell_mark_capture_changed(&prev, &d));
}

#[test]
fn sniff_osc133_edit_mode_does_not_emit_prompt_state() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]133;V;1\x07");
    assert!(
        sig.shell.is_empty(),
        "edit-mode metadata must not bump prompt state or prompt sequence"
    );

    let b = s.feed(b"\x1b]133;B\x07");
    assert!(b.shell.last().unwrap().active);
    assert!(b.shell.last().unwrap().at_prompt);
}

/// The collapse that suppression performs must not hand the pane back a
/// prompt it has already left. A far shell's whole turn can arrive in one
/// read — the previous command's `D`, the prompt's `A`/`B`, then the next
/// command's `C` — and once `at_prompt` is cleared across the batch those
/// two entries differ only in the mark's own reading. The collapse keeps
/// the earlier entry, so that reading has to travel with it or freeness
/// answers `free` while a remote command is running (#840).
#[test]
fn suppression_collapses_a_batch_onto_its_newest_marks_reading() {
    let mut s = OscSniffer::new();
    // Bare `133;C`: nushell's integration emits it with no command name, as
    // do several third-party ones, so `command` cannot tell the two entries
    // apart either.
    let mut turn = s.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07");
    suppress_relayed_prompt_marks(&mut turn.shell);
    assert_eq!(
        turn.shell.len(),
        1,
        "still one `Prompt` message, exactly as before the mark was added"
    );
    assert!(
        !turn.shell.last().unwrap().mark_at_prompt,
        "the newest mark started a command, so the pane is not at a prompt"
    );

    // And the other order: a command that starts and finishes inside one
    // read has to end at the prompt, not at the `C` that opened it.
    let mut turn = s.feed(b"\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07");
    suppress_relayed_prompt_marks(&mut turn.shell);
    assert_eq!(turn.shell.len(), 1);
    assert!(
        turn.shell.last().unwrap().mark_at_prompt,
        "the newest mark is the prompt the far shell just drew"
    );
}

#[test]
fn sniff_resyncs_on_new_osc_after_an_unterminated_one() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    assert!(
        sig.shell.last().is_some_and(|sh| sh.at_prompt),
        "OSC 133;B after an unterminated 133;A was dropped (no resync on `]`)"
    );

    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]7;file://host/dropped\x1b]7;file://host/kept\x07");
    assert_eq!(sig.cwd, Some(PathBuf::from("/kept")));
}

#[test]
fn at_prompt_covers_prompt_draw_gap_across_chunks() {
    let mut s = OscSniffer::new();

    assert!(!s.feed(b"\x1b]133;C\x07").shell.last().unwrap().at_prompt);

    let d = s.feed(b"\x1b]133;D;0\x07");
    assert!(
        d.shell.last().unwrap().at_prompt,
        "D should mark us back at the prompt before the prompt text is drawn"
    );

    let chunk = s.feed(
        b"\x1b]133;A\x07\x1b]7;file://host/repo/lyrel\x07\r\nlyrel git:(main) \xe2\x9e\x9c ",
    );
    assert!(
        chunk.shell.last().unwrap().at_prompt,
        "prompt visible but at_prompt=false — the mis-routing window is still open"
    );

    assert!(s.feed(b"\x1b]133;B\x07").shell.last().unwrap().at_prompt);
}

#[test]
fn parse_osc7_forms_and_rejections() {
    assert_eq!(
        parse_osc7(b"7;file://host/Users/me/dev"),
        Some(PathBuf::from("/Users/me/dev"))
    );
    assert_eq!(parse_osc7(b"7;file:///etc"), Some(PathBuf::from("/etc")));
    assert_eq!(parse_osc7(b"7;/var/log"), Some(PathBuf::from("/var/log")));
    assert_eq!(
        parse_osc7(b"7;file://host/a%20b"),
        Some(PathBuf::from("/a b"))
    );
    assert_eq!(
        parse_osc7(b"7;file://host/%E4%B8%AD%E6%96%87"),
        Some(PathBuf::from("/中文"))
    );
    assert_eq!(
        parse_osc7(b"7;file://host/tmp/a%2520b"),
        Some(PathBuf::from("/tmp/a%20b"))
    );
    assert!(parse_osc7(b"8;file://host/x").is_none());
    assert!(parse_osc7(b"7;file://host").is_none());
    assert!(parse_osc7(b"7;relative/path").is_none());
    assert!(parse_osc7(b"7;file://host").is_none());
}

#[test]
fn strip_uri_drive_slash_only_unwraps_drive_paths() {
    assert_eq!(strip_uri_drive_slash("/C:/Users/foo"), "C:/Users/foo");
    assert_eq!(strip_uri_drive_slash("/d:/x"), "d:/x");
    assert_eq!(strip_uri_drive_slash("/home/me/dev"), "/home/me/dev");
    assert_eq!(strip_uri_drive_slash("//host/share"), "//host/share");
    assert_eq!(strip_uri_drive_slash("C:/already"), "C:/already");
    assert_eq!(strip_uri_drive_slash("/"), "/");
}

#[test]
fn percent_decode_handles_escapes_and_garbage() {
    assert_eq!(percent_decode(b"a%20b"), b"a b");
    assert_eq!(percent_decode(b"%2F"), b"/");
    assert_eq!(percent_decode(b"%2f"), b"/");
    assert_eq!(percent_decode(b"%GG"), b"%GG");
    assert_eq!(percent_decode(b"x%2"), b"x%2");
    assert_eq!(percent_decode(b"plain"), b"plain");
}

#[test]
fn hex_val_ranges() {
    assert_eq!(hex_val(b'0'), Some(0));
    assert_eq!(hex_val(b'9'), Some(9));
    assert_eq!(hex_val(b'a'), Some(10));
    assert_eq!(hex_val(b'f'), Some(15));
    assert_eq!(hex_val(b'A'), Some(10));
    assert_eq!(hex_val(b'F'), Some(15));
    assert!(hex_val(b'g').is_none());
    assert!(hex_val(b' ').is_none());
    assert!(hex_val(b'/').is_none());
}

#[test]
fn osc133_exit_code_parsing() {
    let mut s = OscSniffer::new();
    let d = s.feed(b"\x1b]133;D\x07");
    assert!(d.shell.last().unwrap().at_prompt);
    assert_eq!(d.shell.last().unwrap().last_exit_code, None);

    let d = s.feed(b"\x1b]133;D;oops\x07");
    assert_eq!(d.shell.last().unwrap().last_exit_code, None);

    let d = s.feed(b"\x1b]133;D;-1\x07");
    assert_eq!(d.shell.last().unwrap().last_exit_code, Some(-1));
}

#[test]
fn a_finished_or_waiting_turn_keeps_its_reconnect_and_resume_identity() {
    use crate::core::cli_agent::{AgentStatus, CLIAgent, parse_agent_event_body};

    for (body, expected) in [
        (
            r#"{"agent":"omp","event":"prompt-submit"}"#,
            AgentStatus::Working,
        ),
        (r#"{"agent":"omp","event":"stop"}"#, AgentStatus::Done),
        (
            r#"{"agent":"omp","event":"permission-request","request":{"id":"approval","kind":"permission"}}"#,
            AgentStatus::Waiting,
        ),
    ] {
        let mut state = test_state(true);
        (&mut state).with_facts(|facts, emit| apply_agent(facts, Some((CLIAgent::OhMyPi, vec!["omp".into()])), emit));
        let start = parse_agent_event_body(
            br#"{"agent":"omp","event":"session-start","session_id":"still-running","session_scope":{"session_file":"/isolated/still-running.jsonl"}}"#,
        )
        .unwrap();
        (&mut state).with_facts(|facts, emit| apply_agent_signals(facts, vec![start], None, emit));
        (&mut state).with_facts(|facts, emit| apply_agent_signals(facts, vec![parse_agent_event_body(body.as_bytes()).unwrap()], None, emit));
        let snapshot = (&state).facts.agent_state((&state).id).expect("reconnect keeps a live agent");
        assert_eq!(snapshot.agent, Some(CLIAgent::OhMyPi));
        assert_eq!(snapshot.state.status, expected);
        assert_eq!(snapshot.state.session_id.as_deref(), Some("still-running"));
        let facts = observed_facts(&(&state).facts)
            .agent
            .expect("resume identity is kept");
        assert_eq!(facts.session_id, snapshot.state.session_id);
        assert_eq!(facts.launch_argv, Some(vec!["omp".into()]));
        assert!(facts.session_scope.is_some());
        let (tx, rx) = mpsc::channel();
        attach_subscriber_with_permissions(&mut state, tx, &OutputGate::new(), false, true);
        let replay: Vec<_> = rx.try_iter().collect();
        assert!(
            replay
                .iter()
                .any(|msg| matches!(msg, DaemonMsg::Agent(Some(CLIAgent::OhMyPi))))
        );
        assert!(replay.iter().any(|msg| matches!(
            msg,
            DaemonMsg::AgentStatus(Some(session)) if session == &snapshot.state
        )));
    }
}

#[test]
fn opaque_notifications_only_fall_back_when_no_rich_state() {
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let mut st = test_state(true);
    let mut sniffer = OscSniffer::new();
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(b"\x1b]9;Build finished\x07"), emit));
    assert!(st.facts.agent_session.is_none());

    st.facts.agent = Some(CLIAgent::Pi);
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(b"\x1b]9;Pi wants to run tests\x07"), emit));
    let sess = st.facts.agent_session.clone().unwrap();
    assert_eq!(sess.status, AgentStatus::Waiting);
    assert!(!sess.rich);

    st.facts.agent_session = Some(AgentSessionState {
        status: AgentStatus::Working,
        message: None,
        session_id: Some("sid".into()),
        launch_argv: None,
        rich: true,
        cwd: None,
        activity: 0,
        turns: 0,
        pending_requests: Vec::new(),
        request_resume_status: AgentStatus::Idle,
        pending_stop: false,
        readout: Default::default(),
    });
    (&mut st).with_facts(|facts, emit| apply_signals(facts, sniffer.feed(b"\x1b]9;noise\x07"), emit));
    assert_eq!(
        st.facts.agent_session.as_ref().unwrap().status,
        AgentStatus::Working
    );
}


/// Issue #711. A pane whose shell printed its prompt and then handed the
/// terminal to a full-screen program keeps `at_prompt` set for as long as
/// that program runs — nothing clears the flag but a `133;C` the program
/// has no reason to send. The live path already refuses to believe a
/// prompt mark that arrives while a foreground command owns the pty; the
/// replay used to re-assert the stored value regardless.
///
/// What the client does with "at a prompt" is scrub the TUI modes it finds
/// in its grid — the alternate screen first (`\x1b[?1049l`) — because a
/// prompting shell means no full-screen program can own the pane. On a
/// replay that alternate screen is the one the ring just rebuilt, so the
/// scrub swapped it away and left the primary screen underneath: the shell
/// banner and the command line the pane was born with, with the program
/// still painting differential updates into a grid that no longer holds
/// what they are differences from.
#[test]
fn a_replay_does_not_claim_a_prompt_while_a_program_owns_the_pane() {
    let mut st = test_state(true);
    // The shell prompted, then `claude` took the terminal and entered the
    // alternate screen. No `133;C` ever arrived, so the flag still stands.
    st.facts.shell = ShellState {
        active: true,
        at_prompt: true,
        // The mark said the same thing; nothing has superseded it.
        mark_at_prompt: true,
        last_exit_code: Some(0),
        command: None,
    };
    st.ring.append(b"\x1b[?1049h\x1b[2Jthe agent's screen");

    let (tx, rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, true);

    let replayed = drain(&rx);
    let prompt = replayed
        .iter()
        .find_map(|msg| match msg {
            DaemonMsg::Prompt {
                active, at_prompt, ..
            } => Some((*active, *at_prompt)),
            _ => None,
        })
        .expect("shell integration is on, so the replay reports the prompt state");
    assert_eq!(
        prompt,
        (true, false),
        "a pane with a program in the foreground is not at a prompt, whatever the last \
         OSC 133 mark said; claiming otherwise costs the client the screen the ring just \
         replayed"
    );
}

/// The other half of the same gate, and the reason it is a gate rather
/// than a deletion: an `ssh` that died mid-`vim` leaves `?1049h` in the
/// ring with no `?1049l` behind it, and the host shell's next prompt is
/// the only thing that says the alternate screen is stale. With nothing in
/// the foreground the report goes out as it always did, and a re-attach
/// still heals that pane.
#[test]
fn a_replay_still_reports_a_prompt_when_the_shell_owns_the_pane() {
    let mut st = test_state(true);
    st.facts.shell = ShellState {
        active: true,
        at_prompt: true,
        // The mark said the same thing; nothing has superseded it.
        mark_at_prompt: true,
        last_exit_code: Some(0),
        command: None,
    };
    st.ring.append(b"\x1b[?1049hstranded alt screen");

    let (tx, rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, false);

    assert!(
        drain(&rx).iter().any(|msg| matches!(
            msg,
            DaemonMsg::Prompt {
                active: true,
                at_prompt: true,
                ..
            }
        )),
        "with no foreground command the stored prompt state is the truth, and the client \
         needs it to leave a stranded alternate screen"
    );
}


#[test]
fn apply_signals_updates_state() {
    let mut st = test_state(true);

    (&mut st).with_facts(|facts, emit| apply_signals(facts, SniffSignals {
        cwd: Some(PathBuf::from("/tmp/x")),
        ..SniffSignals::default()
    }, emit));
    assert_eq!(st.facts.cwd, Some(PathBuf::from("/tmp/x")));

    (&mut st).with_facts(|facts, emit| apply_signals(facts, SniffSignals {
        shell: vec![ShellState {
            active: true,
            at_prompt: true,
            mark_at_prompt: true,
            last_exit_code: Some(0),
            command: None,
        }],
        ..SniffSignals::default()
    }, emit));
    assert!(st.facts.shell.active && st.facts.shell.at_prompt);
    assert_eq!(st.facts.shell.last_exit_code, Some(0));

    (&mut st).with_facts(|facts, emit| apply_signals(facts, SniffSignals::default(), emit));
    assert_eq!(st.facts.cwd, Some(PathBuf::from("/tmp/x")));
}
