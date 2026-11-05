use super::*;

pub(super) fn detached_theme_queries_obey_hint_and_client_ownership(daemon: &Daemon) {
    exercise_detached_queries(daemon, false);
}

#[cfg(unix)]
pub(super) fn detached_theme_queries_survive_public_handoff_refold(daemon: &Daemon) {
    exercise_detached_queries(daemon, true);
}

fn exercise_detached_queries(daemon: &Daemon, refold: bool) {
    #[cfg(windows)]
    assert!(!refold, "Windows does not offer process handoff");
    let shell = ShellSpec {
        program: std::env::current_exe().unwrap().display().to_string(),
        args: vec![THEME_PTY_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut session = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            None,
            None,
        )
        .unwrap();
    session
        .set_recv_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    collect_pane_output_until(&mut session, b"THEME_READY");
    let pane = session.pane_id();
    session.detach().unwrap();
    let address = format!("%{pane}");
    let mut serial = 0;
    let mut query = |bytes: &[u8], replies: usize, expected: &[u8]| {
        serial += 1;
        let command = format!(
            "{serial} {replies} {}{}",
            terminal_pty_fixture::hex(bytes),
            if cfg!(windows) { "\r" } else { "\n" }
        );
        daemon.run_ok(&["send", &address, &command]);
        if replies == usize::MAX {
            let gate = daemon.config_dir.join(format!("theme-fragment-{serial}"));
            let deadline = Instant::now() + Duration::from_secs(10);
            while !gate.exists() {
                assert!(
                    Instant::now() < deadline,
                    "PTY fragment {serial} was not flushed"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            return;
        }
        let marker = format!("THEME_RESULT_{serial} ");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let output = daemon.run_ok(&["capture", &address, "--scrollback"]);
            if let Some((_, result)) = output.split_once(&marker)
                && let Some((reply, _)) = result.split_once(" END")
            {
                assert_eq!(
                    reply,
                    terminal_pty_fixture::hex(expected),
                    "serial {serial}: {output}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "PTY query {serial} never completed: {output}; raw={:?}",
                daemon.run_ok(&["capture", &address, "--scrollback"])
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // No GUI has ever written the hint: lyrel's established default is light.
    query(b"\x1b[?996n", 1, b"\x1b[?997;2n");
    #[cfg(unix)]
    if refold {
        query(b"\x1b[c", 1, b"\x1b[?6c");
        // Explicit manual handoff, not automatic GUI startup adoption.
        let client = PaneClient::at(daemon.pane_endpoint());
        let before = client.version().unwrap().instance;
        client.hand_off(&std::env::current_exe().unwrap()).unwrap();
        daemon.await_ready();
        assert_ne!(client.version().unwrap().instance, before);
        query(b"\x1b[0m", 0, b"");
        query(b"\x1b[?996n", 1, b"\x1b[?997;2n");
        return;
    }
    // Existing DA1 behavior uses the same real daemon/PTY boundary.
    if cfg!(unix) {
        query(b"\x1b[c\x1b[0c", 2, b"\x1b[?6c\x1b[?6c");
    } else {
        eprintln!(
            "Windows DA1 application receipt is not established; use the raw Unix PTY case (native daemon trace counted and wrote both replies successfully)"
        );
    }
    let hint = daemon.config_dir.join("appearance.json");
    std::fs::write(&hint, r#"{"dark":true}"#).unwrap();
    query(b"\x1b[?996n\x1b[?996n", 2, b"\x1b[?997;1n\x1b[?997;1n");
    std::fs::write(&hint, r#"{"dark":false}"#).unwrap();
    query(b"\x1b[?99", usize::MAX, b"");
    query(b"6n", 1, b"\x1b[?997;2n");
    std::fs::write(&hint, "not json").unwrap();
    query(b"\x1b[?996n", 1, b"\x1b[?997;2n");

    let attached = PaneClient::at(daemon.pane_endpoint())
        .attach(
            pane,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
        )
        .unwrap();
    // A replay itself must not manufacture a second application reply.
    query(b"\x1b[0m", 0, b"");
    // No emulator is answering here: a responsible client suppresses daemon
    // replies, even though the stored hint is dark.
    std::fs::write(&hint, r#"{"dark":true}"#).unwrap();
    query(b"\x1b[?996n\x1b[c", 0, b"");
    query(b"\x1b[?99", usize::MAX, b"");
    attached.detach().unwrap();
    query(b"6n", 1, b"\x1b[?997;1n");
    if cfg!(unix) {
        query(b"\x1b[c", 1, b"\x1b[?6c");
    }
}
