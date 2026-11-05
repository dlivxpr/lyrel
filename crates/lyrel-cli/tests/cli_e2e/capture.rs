use super::*;

/// `--plain` against a real pane, end to end through a real daemon.
///
/// The discriminator is the PTY's own line ending: a terminal ends lines with
/// CRLF, so every raw capture carries `\r`, and a rendered one carries none —
/// that CR was an instruction to the grid, not text. It holds whatever the test
/// machine's shell decorates its prompt with, which a check for escape bytes
/// would not: the isolated daemon's shell prints no colour at all.
///
/// What the grid *does* with those bytes (wraps, overwrites, cursor moves) is
/// pinned by the unit tests in `screen.rs`, which can craft the byte stream
/// exactly. This one proves the flag reaches them.
pub(super) fn capture_plain_returns_text_not_escapes(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    let address = format!("%{pane}");

    daemon.run_ok(&["send", &address, "echo lyrel_e2e_plain_marker", "--enter"]);

    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let raw = daemon.run_ok(&["capture", &address, "--scrollback"]);
        let plain = daemon.run_ok(&["capture", &address, "--scrollback", "--plain"]);
        // The marker renders as soon as the shell echoes the typed command,
        // which can be before the pane has seen a single CR — ConPTY repaints
        // the input line in escape-laden bursts, and `raw` is a separate,
        // slightly earlier snapshot besides. The CR is part of what must
        // settle, not something the marker's arrival already proves.
        if plain.contains("lyrel_e2e_plain_marker") && raw.contains('\r') {
            assert!(
                !plain.contains('\r'),
                "a carriage return is an instruction to the grid, not text:\n{plain:?}"
            );
            assert!(
                !plain.contains('\u{1b}'),
                "an escape survived the grid:\n{plain:?}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the captures never settled (marker rendered, CRLF in the raw \
             bytes); last plain was:\n{plain}\nlast raw was:\n{raw:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The only consumers are the pane protocol and the real CLI: no GUI can
/// supply a cached presentation or release the open synchronized update.
pub(super) fn capture_plain_reads_unfinished_synchronized_output_without_a_gui(daemon: &Daemon) {
    let shell = ShellSpec {
        program: std::env::current_exe().unwrap().display().to_string(),
        args: vec![TERMINAL_PTY_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 40,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("synchronized-capture-e2e".into()),
            None,
        )
        .expect("spawn the shared raw PTY sender");
    pane.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
    collect_pane_output_until(&mut pane, b"KITTY_READY");
    let address = format!("%{}", pane.pane_id());
    let mut serial = 0;
    let mut send = |pane: &mut lyrel_core::client::PaneSession, bytes: &[u8]| {
        serial += 1;
        pane.input(format!("SEND {serial} 0 {} 1\n", terminal_pty_fixture::hex(bytes)).as_bytes())
            .unwrap();
        // The child completes a daemon query/reply barrier before DONE. This
        // establishes received/parsed output without a wall-clock sleep, and
        // does not close the synchronized update. Each byte is written and
        // flushed separately; the PTY may coalesce adjacent writes.
        collect_pane_output_until(pane, format!("KITTY_DONE_{serial};\r").as_bytes());
    };

    let text = terminal_pty_fixture::SYNC_CAPTURE_TEXT;
    send(
        &mut pane,
        &terminal_pty_fixture::synchronized_capture_frame(),
    );
    let raw = daemon.run_ok(&["capture", &address, "--scrollback"]);
    // Unix PTYs preserve the marker. ConPTY can consume/rewrite terminal
    // controls before lyrel receives them, so its text result alone is not
    // evidence that a GUI held a 2026 frame. Record that observation boundary.
    if cfg!(unix) {
        assert!(raw.contains("\x1b[?2026h"), "missing begin marker: {raw:?}");
        assert!(
            !raw.contains("\x1b[?2026l"),
            "unexpected end marker: {raw:?}"
        );
    } else if !raw.contains("\x1b[?2026h") {
        eprintln!("ConPTY did not preserve CSI ?2026h; this run proves received-text capture only");
    }
    let live = daemon.run_json(&["capture", &address, "--plain"]);
    let live = live["text"].as_str().expect("live plain capture text");
    assert!(live.lines().any(|line| line == text), "{live:?}");

    // Completion changes eligibility for GUI presentation, not the already
    // received text. Compare the same logical content range, not the fixture's
    // changing barrier row or full scrollback against a viewport.
    send(&mut pane, b"\x1b[?2026l");
    let settled = daemon.run_json(&["capture", &address, "--plain"]);
    let settled = settled["text"]
        .as_str()
        .expect("settled plain capture text");
    assert!(settled.lines().any(|line| line == text), "{settled:?}");

    pane.resize(WinSize {
        cols: 28,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
    })
    .unwrap();
    loop {
        match pane.recv().expect("wait for resize acknowledgement") {
            DaemonMsg::Size(size) if size.cols == 28 && size.rows == 24 => break,
            DaemonMsg::Exited { code } => panic!("sender exited during resize: {code:?}"),
            _ => {}
        }
    }
    let resized = daemon.run_json(&["capture", &address, "--plain", "--scrollback"]);
    let resized = resized["text"]
        .as_str()
        .expect("capture after quiet resize");
    assert!(resized.lines().any(|line| line == text), "{resized:?}");

    let next = terminal_pty_fixture::SYNC_NEXT_TEXT;
    send(
        &mut pane,
        format!("\x1b[?2026h\x1b[8;1H{next}\x1b[?2026l\r\nafter_sync").as_bytes(),
    );
    let continued = daemon.run_json(&["capture", &address, "--plain", "--scrollback"]);
    let continued = continued["text"]
        .as_str()
        .expect("capture after successive update");
    assert!(continued.lines().any(|line| line == next), "{continued:?}");
    assert!(
        continued.lines().any(|line| line == "after_sync"),
        "{continued:?}"
    );

    pane.input(b"EXIT 4294967293 0 00 0\n").unwrap();
    loop {
        if let DaemonMsg::Exited { code } = pane.recv().expect("wait for sender exit") {
            assert_eq!(code, Some(0), "raw PTY sender failed");
            break;
        }
    }
    let _ = pane.detach();
}

/// A resize with no new application output must not empty `capture` (#841).
/// Use the finite PTY producer so shell prompt repaint and synthesized mode
/// restoration do not turn raw-byte concatenation into a false contract.
pub(super) fn capture_still_answers_after_a_resize(daemon: &Daemon) {
    let fixture = std::env::current_exe().expect("locate the finite output fixture");
    let shell = ShellSpec {
        program: fixture.display().to_string(),
        args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 100,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("resize-capture-e2e".into()),
            None,
        )
        .expect("spawn the finite output fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN))
        .expect("bound fixture output reads");
    // The trailing CRLF is output too: resizing before it arrives would put
    // a newline-only segment after the six lines this case must capture.
    collect_pane_output_until(&mut pane, b"lyrel_e2e_tail_line_6\r\n");
    let address = format!("%{}", pane.pane_id());
    pane.resize(WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
    })
    .expect("resize the pane");
    loop {
        match pane.recv().expect("wait for the resize acknowledgement") {
            DaemonMsg::Size(size) if size.cols == 80 && size.rows == 24 => break,
            DaemonMsg::Exited { code } => panic!("fixture exited before capture: {code:?}"),
            _ => {}
        }
    }

    let plain = daemon.run_ok(&["capture", &address, "--plain"]);
    let expected: Vec<String> = (1..=6)
        .map(|n| format!("lyrel_e2e_tail_line_{n}"))
        .collect();
    assert_eq!(plain.lines().collect::<Vec<_>>(), expected);

    daemon.run_ok(&["send", &address, "--enter"]);
    loop {
        if let DaemonMsg::Exited { code } = pane.recv().expect("wait for fixture exit") {
            assert_eq!(code, Some(0), "finite output fixture failed");
            break;
        }
    }
    let _ = pane.detach();
}

/// `--tail` consumes a real PTY replay, including its CRLF, without depending
/// on the runner's shell startup files or the height of its prompt. The finite
/// output producer waits for capture to finish before it exits and is reaped.
pub(super) fn capture_tail_trims_a_real_panes_answer(daemon: &Daemon) {
    let fixture = std::env::current_exe().expect("locate the capture-tail fixture");
    let shell = ShellSpec {
        program: fixture.display().to_string(),
        args: vec![CAPTURE_TAIL_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("capture-tail-e2e".into()),
            None,
        )
        .expect("spawn the finite capture-tail fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN))
        .expect("bound fixture output reads");
    collect_pane_output_until(&mut pane, b"lyrel_e2e_tail_line_6\r\n");
    let address = format!("%{}", pane.pane_id());

    let whole = daemon.run_json(&["capture", &address, "--plain", "--scrollback"]);
    let tail = daemon.run_json(&[
        "capture",
        &address,
        "--plain",
        "--scrollback",
        "--tail",
        "2",
    ]);
    let whole_text = whole["text"].as_str().expect("whole capture text");
    let tail_text = tail["text"].as_str().expect("tail capture text");
    assert!(
        whole_text.ends_with(tail_text),
        "a tail has to be the end of the answer it was cut from:\n\
         tail: {tail_text:?}\nwhole: {whole_text:?}"
    );
    assert_eq!(tail_text.lines().count(), 2, "{tail}");
    assert!(tail_text.contains("lyrel_e2e_tail_line_6"), "{tail}");
    assert!(
        whole_text.contains("lyrel_e2e_tail_line_1")
            && !tail_text.contains("lyrel_e2e_tail_line_1"),
        "two lines must drop the first of six: whole {whole}, tail {tail}"
    );
    // The count describes the replay, not the rendered or trimmed text.
    assert_eq!(tail["bytes"], whole["bytes"], "{whole}\n{tail}");
    assert!(
        tail["bytes"]
            .as_u64()
            .is_some_and(|n| n as usize > tail_text.len()),
        "--tail must not shrink the reported replay size: {tail}"
    );

    daemon.run_ok(&["send", &address, "--enter"]);
    loop {
        match pane.recv().expect("wait for capture-tail fixture exit") {
            DaemonMsg::Exited { code } => {
                assert_eq!(code, Some(0), "capture-tail fixture failed");
                break;
            }
            _ => {}
        }
    }
    let _ = pane.detach();
}
