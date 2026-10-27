use super::*;

pub(super) fn run_paste_aware_fixture() {
    let _raw = raw_mode::enable();
    println!("LYREL_PASTE_AWARE_READY");
    std::io::stdout().flush().expect("flush fixture readiness");

    let mut input = Vec::new();
    let mut last_text_at = None;
    let mut chunk = [0_u8; 256];
    loop {
        let read = std::io::stdin()
            .read(&mut chunk)
            .expect("read fixture input");
        assert_ne!(read, 0, "fixture input ended before Enter");
        let read_at = Instant::now();
        for &byte in &chunk[..read] {
            if byte == b'\r' {
                let submitted = input == PASTE_AWARE_TEXT.as_bytes()
                    && last_text_at.is_some_and(|at| {
                        read_at.saturating_duration_since(at) > PASTE_BURST_WINDOW
                    });
                if submitted {
                    println!("LYREL_PASTE_AWARE_SUBMITTED");
                } else {
                    println!("LYREL_PASTE_AWARE_NOT_SUBMITTED");
                }
                std::io::stdout().flush().expect("flush fixture verdict");
                return;
            }
            input.push(byte);
            last_text_at = Some(read_at);
        }
    }
}

#[cfg(unix)]
mod raw_mode {
    pub struct Guard(libc::termios);

    pub fn enable() -> Guard {
        let mut original = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut original) },
            0,
            "read fixture terminal mode"
        );
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        assert_eq!(
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) },
            0,
            "enable fixture raw mode"
        );
        Guard(original)
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
            }
        }
    }
}

#[cfg(windows)]
mod raw_mode {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };

    pub struct Guard {
        input: HANDLE,
        mode: u32,
    }

    pub fn enable() -> Guard {
        let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut mode = 0;
        assert_ne!(
            unsafe { GetConsoleMode(input, &mut mode) },
            0,
            "read fixture terminal mode"
        );
        let raw = (mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        assert_ne!(
            unsafe { SetConsoleMode(input, raw) },
            0,
            "enable fixture raw mode"
        );
        Guard { input, mode }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                SetConsoleMode(self.input, self.mode);
            }
        }
    }
}

pub(super) fn send_then_capture_round_trip(daemon: &Daemon) {
    let created = daemon.run_json(&["new", &workdir()]);
    let pane = created["pane"].as_u64().expect("new prints the pane id");
    let address = format!("%{pane}");

    daemon.run_ok(&["send", &address, "echo lyrel_e2e_capture_marker", "--enter"]);

    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let seen = daemon.run_ok(&["capture", &address, "--scrollback"]);
        if seen.contains("lyrel_e2e_capture_marker") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the sent text never showed up in the capture; last capture:\n{seen}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub(super) fn send_enter_submits_in_a_paste_aware_raw_mode_tui(daemon: &Daemon) {
    let fixture = std::env::current_exe().expect("locate the paste-aware fixture");
    let shell = ShellSpec {
        program: fixture.display().to_string(),
        args: vec![PASTE_AWARE_FIXTURE_ARG.into()],
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
            Some("paste-aware-e2e".into()),
            None,
        )
        .expect("spawn the paste-aware raw-mode fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN))
        .expect("bound fixture output reads");
    collect_pane_output_until(&mut pane, b"LYREL_PASTE_AWARE_READY");

    let address = format!("%{}", pane.pane_id());
    daemon.run_ok(&["send", &address, PASTE_AWARE_TEXT, "--enter"]);

    collect_pane_output_until(&mut pane, b"LYREL_PASTE_AWARE_SUBMITTED");
}

pub(super) fn send_via_stdin(daemon: &Daemon, args: &[&str], input: &[u8]) -> Output {
    let mut child = daemon
        .cli(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("could not run lyrel {args:?}: {e}"));
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input)
        .expect("feed lyrel's stdin");
    child.wait_with_output().expect("lyrel ran to completion")
}

pub(super) fn send_stdin_delivers_bytes_it_never_echoes(daemon: &Daemon) {
    if cfg!(windows) {
        return;
    }
    let pane = bare_pane(daemon, "/bin/cat");
    let address = format!("%{pane}");
    let payload = "lyrel_e2e_stdin_secret\n";
    let out = send_via_stdin(
        daemon,
        &["send", &address, "--stdin", "--json"],
        payload.as_bytes(),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !stdout.contains(payload.trim_end()),
        "the payload must not come back in the report: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("--json prints JSON");
    assert_eq!(json["source"], "stdin", "{json}");
    assert_eq!(json["bytes"].as_u64(), Some(payload.len() as u64), "{json}");
    await_capture(daemon, &address, payload.trim_end());
}

pub(super) fn send_paste_asks_the_pane_for_its_bracketed_paste_mode(daemon: &Daemon) {
    if cfg!(windows) {
        return;
    }
    let pane = bare_pane(daemon, "/bin/cat");
    let address = format!("%{pane}");

    // Off to begin with: nothing in the pane has asked for it.
    let json = daemon.run_json(&["send", &address, "lyrel_e2e_one\nlyrel_e2e_two", "--paste"]);
    assert_eq!(json["bracketed_paste_mode"], false, "{json}");
    assert_eq!(json["bracketed"], false, "{json}");
    await_capture(daemon, &address, "lyrel_e2e_two");

    // `cat` hands back what it reads, so the program in the pane is now the
    // one that switched mode 2004 on — the daemon sees it in the output.
    let out = send_via_stdin(daemon, &["send", &address, "--stdin"], b"\x1b[?2004h\n");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let procs = daemon.run_json(&["procs", &address]);
        if procs["context"]["bracketed_paste"].as_bool() == Some(true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pane never reported bracketed paste on: {procs}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let json = daemon.run_json(&["send", &address, "lyrel_e2e_framed", "--paste"]);
    assert_eq!(json["bracketed"], true, "{json}");
    // The frame reached the pty: cat's echo of it carries the opener's tail.
    let seen = await_capture(daemon, &address, "lyrel_e2e_framed");
    assert!(seen.contains("[200~"), "{seen:?}");
}
