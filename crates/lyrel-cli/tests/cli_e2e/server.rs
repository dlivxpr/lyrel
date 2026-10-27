use super::*;

pub(super) fn config_dir_alone_resolves_both_endpoints(daemon: &Daemon) {
    let out = Command::new(env!("CARGO_BIN_EXE_lyrel"))
        .args(["status", "--json"])
        .env_remove("LYREL_DATA_DIR")
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_PANE")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_SOCKET")
        .env("LYREL_CONFIG_DIR", &daemon.config_dir)
        .output()
        .expect("run lyrel status with only LYREL_CONFIG_DIR");
    assert!(
        out.status.success(),
        "status over LYREL_CONFIG_DIR failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let status: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("status --json prints one JSON object");
    assert_eq!(
        status["pid"].as_u64(),
        Some(u64::from(daemon.child.id())),
        "the answer must come from the daemon LYREL_CONFIG_DIR names: {status}"
    );

    // The control endpoint alone proves nothing: the bug this pins had `status`
    // working while every pane verb reached the wrong socket, because the two
    // endpoints were derived by different rules. Exercise a pane verb over the
    // same lone variable.
    let shell = one_shot(if cfg!(windows) {
        "echo %TERM_PROGRAM%^|%LYREL_CONFIG_DIR% & exit /b 9"
    } else {
        "printf '%s|%s\\n' \"$TERM_PROGRAM\" \"$LYREL_CONFIG_DIR\"; exit 9"
    });
    let mut args = vec!["run", "--"];
    args.extend(shell.iter().map(String::as_str));
    let out = Command::new(env!("CARGO_BIN_EXE_lyrel"))
        .args(args)
        .env_remove("LYREL_DATA_DIR")
        .env_remove("LYREL_CONTROL_SOCK")
        .env_remove("LYREL_PANE")
        .env_remove("LYREL_WS")
        .env_remove("LYREL_SOCKET")
        .env("LYREL_CONFIG_DIR", &daemon.config_dir)
        .output()
        .expect("run lyrel run with only LYREL_CONFIG_DIR");
    assert_eq!(
        out.status.code(),
        Some(9),
        "a pane verb must reach the same server the control verb did: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("lyrel|{}", daemon.config_dir.display())),
        "the actual shell must inherit the new product and config identity: {stdout}"
    );
}

// A real daemon stands in for a separately installed other product. Only its
// own environment advertises it; discovering or stopping it is never allowed.
pub(super) fn legacy_environment_does_not_discover_or_stop_server(daemon: &Daemon) {
    let legacy = Daemon::start_with_layout(Some(if cfg!(windows) {
        "otherterm"
    } else {
        ".config/otherterm"
    }));
    let home = legacy.dir.path();
    let sentinel = legacy.config_dir.join("config.json");
    std::fs::write(&sentinel, b"{\"old_product_fixture\":true}")
        .expect("write legacy data sentinel");
    legacy.run_ok(&["ws", "new", "legacy-workspace"]);
    let before = legacy.run_json(&["ls"]);

    let command = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lyrel"));
        cmd.args(args)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("APPDATA", home)
            .env("OTHERTERM_CONFIG_DIR", &legacy.config_dir)
            .env("OTHERTERM_DATA_DIR", &legacy.config_dir)
            .env("OTHERTERM_CONTROL_SOCK", legacy.control_endpoint())
            .env("OTHERTERM_SOCKET", legacy.pane_endpoint())
            .env_remove("LYREL_CONFIG_DIR")
            .env_remove("LYREL_DATA_DIR")
            .env_remove("LYREL_CONTROL_SOCK")
            .env_remove("LYREL_SOCKET")
            .env_remove("LYREL_PANE")
            .env_remove("LYREL_WS");
        cmd
    };
    let ours = daemon
        .cli(&["status", "--json"])
        .env("OTHERTERM_CONFIG_DIR", &legacy.config_dir)
        .env("OTHERTERM_DATA_DIR", &legacy.config_dir)
        .env("OTHERTERM_CONTROL_SOCK", legacy.control_endpoint())
        .env("OTHERTERM_SOCKET", legacy.pane_endpoint())
        .output()
        .expect("resolve new identity beside the legacy fixture");
    assert!(ours.status.success(), "{ours:?}");
    let ours: serde_json::Value =
        serde_json::from_slice(&ours.stdout).expect("new identity status returns JSON");
    assert_eq!(ours["pid"].as_u64(), Some(u64::from(daemon.child.id())));

    let status = command(&["status", "--json"])
        .output()
        .expect("probe default discovery with only legacy environment");
    assert!(
        !status.status.success(),
        "the old daemon must not answer new-product default discovery: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    let stop = command(&["server", "stop", "--json"])
        .output()
        .expect("stop with only legacy environment");
    assert!(stop.status.success(), "{stop:?}");
    let result: serde_json::Value =
        serde_json::from_slice(&stop.stdout).expect("stop returns JSON");
    assert_eq!(result["stopped"], false, "{result}");
    assert_eq!(legacy.run_json(&["ls"]), before);
    assert_eq!(
        std::fs::read(&sentinel).expect("legacy configuration survives"),
        b"{\"old_product_fixture\":true}"
    );
}

pub(super) fn status_reports_the_live_server(daemon: &Daemon) {
    let status = daemon.run_json(&["status"]);
    assert!(
        status["pid"].as_u64().is_some_and(|pid| pid > 0),
        "{status}"
    );
    assert_eq!(
        status["protocol_version"].as_u64(),
        Some(u64::from(PROTOCOL_VERSION)),
        "{status}"
    );

    let human = daemon.run_ok(&["server", "status"]);
    assert!(human.contains("pid"), "{human}");
}

pub(super) fn events_stream_reports_a_workspace_creation(daemon: &Daemon) {
    let mut watcher = daemon
        .cli(&["events", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start lyrel events");
    let stdout = watcher.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + SETTLE_WITHIN;
    let mut seen = Vec::new();
    let verdict = 'outer: loop {
        if Instant::now() >= deadline {
            break false;
        }
        daemon.run_ok(&["ws", "new", "evtws"]);
        let round = Instant::now() + Duration::from_secs(5);
        while let Ok(line) = rx.recv_timeout(round.saturating_duration_since(Instant::now())) {
            let is_event = serde_json::from_str::<serde_json::Value>(&line).is_ok();
            seen.push(line);
            if is_event {
                break 'outer true;
            }
        }
    };
    let _ = watcher.kill();
    let _ = watcher.wait();
    assert!(
        verdict,
        "no event line arrived within {SETTLE_WITHIN:?}; saw {seen:?}"
    );
}
