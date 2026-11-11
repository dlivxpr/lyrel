// Public seam: interactive PTY -> native child environment -> next prompt.
// No installed agents, network, user home or renderer mocks are involved.
#[test]
#[ignore = "owned child process used by interactive shell environment contracts"]
fn scope_environment_observer() {
    let Some(root) = std::env::var_os("LYREL_CAPTURE_ROOT") else { return; };
    let label = std::env::var("LYREL_CAPTURE_LABEL").unwrap_or_else(|_| "child".into());
    let environment: std::collections::BTreeMap<_, _> = [
        "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "LC_TERMINAL", "LC_TERMINAL_VERSION",
        "PI_NO_DECCARA", "KITTY_WINDOW_ID", "PI_IMAGE_PROTOCOL", "PI_FORCE_IMAGE_PROTOCOL",
        "PI_KITTY_PLACEHOLDERS", "PI_NO_KITTY_PLACEHOLDERS", "OPENCODE_CLI_CONFIG_CONTENT",
    ].into_iter().map(|key| (key, std::env::var(key).ok())).collect();
    std::fs::write(Path::new(&root).join(format!("{label}.json")), serde_json::to_vec(&environment).unwrap()).unwrap();
    if label == "interrupt" {
        println!("LYREL_INTERRUPT_READY");
        use std::io::Write;
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
    }
    if label == "failure" { std::process::exit(1); }
}

#[cfg(unix)]
fn scope_shell_contract(program: &str) {
    let available = std::process::Command::new(program).arg("--version").output();
    if matches!(&available, Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
        eprintln!("skipping: {program} not installed");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin with space");
    std::fs::create_dir(&bin).unwrap();
    for name in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes", "ordinary"] {
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), bin.join(name)).unwrap();
    }
    let observer = "--ignored --exact daemon::shell_integration::tests::scope_environment_observer --nocapture";
    let shell = Path::new(program).file_name().unwrap().to_str().unwrap();
    let mut injection = match shell {
        "bash" => {
            let rc = home.path().join("bashrc");
            std::fs::write(&rc, BASH_INTEGRATION).unwrap();
            Injection { env: HashMap::new(), args: vec!["--noprofile".into(), "--rcfile".into(), rc.to_string_lossy().into_owned(), "-i".into()], replaces_argv: true, dir: None }
        }
        "zsh" => {
            let mut injection = setup_zsh().unwrap();
            injection.env.insert("LYREL_USER_ZDOTDIR".into(), home.path().to_string_lossy().into_owned());
            injection.args.push("-i".into());
            injection
        }
        "fish" => {
            let mut injection = setup_fish().unwrap();
            injection.args.push("-i".into());
            injection
        }
        _ => panic!("not a POSIX shell"),
    };
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    injection.env.extend([
        ("PATH".into(), path.to_string_lossy().into_owned()),
        ("HOME".into(), home.path().to_string_lossy().into_owned()),
        ("XDG_CONFIG_HOME".into(), home.path().to_string_lossy().into_owned()),
        ("LYREL_CAPTURE_ROOT".into(), home.path().to_string_lossy().into_owned()),
        ("LYREL_SHELL_INTEGRATION".into(), String::new()),
        ("LYREL".into(), env!("CARGO_PKG_VERSION").into()),
        ("TERM_PROGRAM".into(), "lyrel".into()),
        ("TERM_PROGRAM_VERSION".into(), "original-version".into()),
        ("LC_TERMINAL".into(), String::new()),
        ("LC_TERMINAL_VERSION".into(), "original-locale-version".into()),
        ("PI_FORCE_IMAGE_PROTOCOL".into(), "off".into()),
        ("PI_IMAGE_PROTOCOL".into(), "none".into()),
        ("PI_KITTY_PLACEHOLDERS".into(), "0".into()),
        ("PI_NO_KITTY_PLACEHOLDERS".into(), "1".into()),
        ("OPENCODE_CLI_CONFIG_CONTENT".into(), "{\"session\":{\"image_preview\":false}}".into()),
    ]);
    let mut lines = vec![if shell == "fish" {
        "set -e PI_NO_DECCARA; set -e KITTY_WINDOW_ID\n".into()
    } else { "unset PI_NO_DECCARA KITTY_WINDOW_ID\n".into() }];
    for agent in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes"] {
        lines.push(format!("LYREL_CAPTURE_LABEL={agent} {agent} {observer}\n"));
        lines.push(format!("LYREL_CAPTURE_LABEL=after-{agent} ordinary {observer}\n"));
    }
    lines.extend([
        format!("LYREL_CAPTURE_LABEL=absolute '{}' {observer}\n", bin.join("omp").display()),
        format!("TERM_PROGRAM=explicit TERM_PROGRAM_VERSION=chosen LYREL_CAPTURE_LABEL=explicit omp {observer}\n"),
        format!("TERM_PROGRAM='' LYREL_CAPTURE_LABEL=empty-identity omp {observer}\n"),
        format!("KITTY_WINDOW_ID='' LYREL_CAPTURE_LABEL=empty-marker hermes {observer}\n"),
        format!("KITTY_WINDOW_ID=owned LYREL_CAPTURE_LABEL=owned-marker hermes {observer}\n"),
        format!("LYREL_CAPTURE_LABEL=failure omp {observer}\n"),
        format!("LYREL_CAPTURE_LABEL=after-failure ordinary {observer}\n"),
        format!("LYREL_CAPTURE_LABEL=interrupt omp {observer}\n"),
        "\u{3}".into(),
        format!("LYREL_CAPTURE_LABEL=after-interrupt ordinary {observer}\n"),
        format!("LYREL_CAPTURE_LABEL=background omp {observer} &\n"),
        "wait\n".into(),
        format!("LYREL_CAPTURE_LABEL=after-background ordinary {observer}\n"),
        format!("LYREL_CAPTURE_LABEL=pipeline omp {observer} | cat\n"),
        format!("LYREL_CAPTURE_LABEL=compound omp {observer}; true\n"),
        format!("LYREL_CAPTURE_LABEL=interpreter sh -c 'omp {observer}'\n"),
        "false\n".into(),
    ]);
    let steps: Vec<_> = lines.iter().map(|line| line.as_bytes()).collect();
    let text = prompt_steps_over_pty(program, &injection, &steps, Some(home.path()));
    for label in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes", "absolute", "failure", "interrupt", "background"] {
        let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap_or_else(|error| panic!("{label}: {error}\n{text}"))).unwrap();
        assert_eq!(actual["TERM_PROGRAM"], "kitty", "{shell}/{label}: {text}");
        assert_eq!(actual["TERM_PROGRAM_VERSION"], "");
        assert_eq!(actual["LC_TERMINAL"], "lyrel");
        assert_eq!(actual["LC_TERMINAL_VERSION"], env!("CARGO_PKG_VERSION"));
        let is_omp = matches!(label, "omp" | "absolute" | "failure" | "interrupt" | "background");
        assert_eq!(actual["PI_NO_DECCARA"], if is_omp { serde_json::json!("1") } else { serde_json::Value::Null });
        assert_eq!(actual["KITTY_WINDOW_ID"], if label == "hermes" { serde_json::json!("lyrel") } else { serde_json::Value::Null });
        assert_eq!(actual["PI_FORCE_IMAGE_PROTOCOL"], "off");
        assert_eq!(actual["PI_IMAGE_PROTOCOL"], "none");
        assert_eq!(actual["PI_KITTY_PLACEHOLDERS"], "0");
        assert_eq!(actual["PI_NO_KITTY_PLACEHOLDERS"], "1");
        assert_eq!(actual["OPENCODE_CLI_CONFIG_CONTENT"], "{\"session\":{\"image_preview\":false}}");
    }
    for agent in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes"] {
        let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("after-{agent}.json"))).unwrap()).unwrap();
        assert_eq!(actual["TERM_PROGRAM"], "lyrel");
        assert_eq!(actual["TERM_PROGRAM_VERSION"], "original-version");
        assert_eq!(actual["LC_TERMINAL"], "");
        assert_eq!(actual["LC_TERMINAL_VERSION"], "original-locale-version");
        assert_eq!(actual["PI_NO_DECCARA"], serde_json::Value::Null);
        assert_eq!(actual["KITTY_WINDOW_ID"], serde_json::Value::Null);
    }
    for label in ["after-failure", "after-interrupt", "after-background", "pipeline", "compound", "interpreter"] {
        let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap()).unwrap();
        assert_eq!(actual["TERM_PROGRAM"], "lyrel", "{shell}/{label}: {text}");
        assert_eq!(actual["TERM_PROGRAM_VERSION"], "original-version");
        assert_eq!(actual["LC_TERMINAL"], "");
        assert_eq!(actual["LC_TERMINAL_VERSION"], "original-locale-version");
    }
    let explicit: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join("explicit.json")).unwrap()).unwrap();
    assert_eq!(explicit["TERM_PROGRAM"], "explicit");
    assert_eq!(explicit["TERM_PROGRAM_VERSION"], "chosen");
    for (label, key, expected) in [
        ("empty-identity", "TERM_PROGRAM", ""),
        ("empty-marker", "KITTY_WINDOW_ID", ""),
        ("owned-marker", "KITTY_WINDOW_ID", "owned"),
    ] {
        let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap()).unwrap();
        assert_eq!(actual[key], expected, "{shell}/{label}: {text}");
    }
    if let Some(dir) = injection.dir { std::fs::remove_dir_all(dir).unwrap(); }
}

#[cfg(unix)]
#[test]
fn bash_manual_agent_scope_and_restoration_over_a_real_pty() {
    scope_shell_contract(&std::env::var("LYREL_TEST_BASH").unwrap_or_else(|_| "/bin/bash".into()));
}

#[cfg(unix)]
#[test]
fn zsh_manual_agent_scope_and_restoration_over_a_real_pty() {
    scope_shell_contract("zsh");
}

#[cfg(unix)]
#[test]
fn fish_manual_agent_scope_and_restoration_over_a_real_pty() {
    scope_shell_contract("fish");
}

fn structured_scope_shell_contract(program: &str, nushell: bool) {
    let available = if nushell {
        std::process::Command::new(program).arg("--version").output()
    } else {
        std::process::Command::new(program).args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.Major"]).output()
    };
    let version = match available {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: {program} not installed");
            return;
        }
        result => result.unwrap(),
    };
    let powershell_major = if nushell { 0 } else {
        assert!(version.status.success(), "{program}: {}", String::from_utf8_lossy(&version.stderr));
        String::from_utf8_lossy(&version.stdout).trim().parse::<u32>().unwrap()
    };
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin with space");
    std::fs::create_dir(&bin).unwrap();
    for name in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes", "ordinary"] {
        let name = if cfg!(windows) { format!("{name}.exe") } else { name.into() };
        std::fs::copy(std::env::current_exe().unwrap(), bin.join(name)).unwrap();
    }
    let mut injection = if nushell { setup_nushell_with(None).unwrap() } else {
        let mut injection = setup_powershell().unwrap();
        injection.args.insert(0, "-NoProfile".into());
        injection
    };
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    injection.env.extend([
        ("PATH".into(), path.to_string_lossy().into_owned()),
        ("LYREL_CAPTURE_ROOT".into(), home.path().to_string_lossy().into_owned()),
        ("LYREL_SHELL_INTEGRATION".into(), String::new()),
        ("LYREL".into(), env!("CARGO_PKG_VERSION").into()),
        ("TERM_PROGRAM".into(), "lyrel".into()),
        ("TERM_PROGRAM_VERSION".into(), "original-version".into()),
        ("LC_TERMINAL".into(), String::new()),
        ("PI_IMAGE_PROTOCOL".into(), "none".into()),
        ("PI_FORCE_IMAGE_PROTOCOL".into(), "off".into()),
        ("PI_KITTY_PLACEHOLDERS".into(), "0".into()),
        ("PI_NO_KITTY_PLACEHOLDERS".into(), "1".into()),
        ("OPENCODE_CLI_CONFIG_CONTENT".into(), "{\"session\":{\"image_preview\":false}}".into()),
    ]);
    let observer = "--ignored --exact daemon::shell_integration::tests::scope_environment_observer --nocapture";
    let call = |label: &str, command: &str| if nushell {
        format!("with-env {{LYREL_CAPTURE_LABEL: '{label}'}} {{ {command} {observer} }}\r")
    } else { format!("$env:LYREL_CAPTURE_LABEL='{label}'; {command} {observer}\r") };
    let mut lines = vec![if nushell {
        "hide-env -i PI_NO_DECCARA KITTY_WINDOW_ID LC_TERMINAL_VERSION\r".into()
    } else { "Remove-Item Env:PI_NO_DECCARA,Env:KITTY_WINDOW_ID,Env:LC_TERMINAL_VERSION -ErrorAction Ignore\r".into() }];
    for agent in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes"] {
        lines.push(call(agent, agent));
        lines.push(call(&format!("after-{agent}"), "ordinary"));
    }
    let explicit = if nushell {
        format!("with-env {{TERM_PROGRAM: 'explicit', TERM_PROGRAM_VERSION: 'chosen', LYREL_CAPTURE_LABEL: 'explicit'}} {{ omp {observer} }}\r")
    } else { format!("$env:TERM_PROGRAM='explicit'; $env:TERM_PROGRAM_VERSION='chosen'; $env:LYREL_CAPTURE_LABEL='explicit'; omp {observer}\r") };
    lines.push(explicit);
    lines.push(if nushell {
        format!("with-env {{TERM_PROGRAM: '', LYREL_CAPTURE_LABEL: 'empty-identity'}} {{ omp {observer} }}\r")
    } else { format!("$env:TERM_PROGRAM=''; $env:LYREL_CAPTURE_LABEL='empty-identity'; omp {observer}\r") });
    lines.push(call("after-empty-identity", "ordinary"));
    lines.push(if nushell {
        format!("with-env {{KITTY_WINDOW_ID: '', LYREL_CAPTURE_LABEL: 'empty-marker'}} {{ hermes {observer} }}\r")
    } else { format!("$env:KITTY_WINDOW_ID=''; $env:LYREL_CAPTURE_LABEL='empty-marker'; hermes {observer}\r") });
    lines.push(call("after-empty-marker", "ordinary"));
    if !nushell {
        lines.push(format!("$env:TERM_PROGRAM_VERSION=''; $env:LC_TERMINAL=''; $env:LC_TERMINAL_VERSION=''; $env:PI_NO_DECCARA=''; $env:OPENCODE_CLI_CONFIG_CONTENT=''; $env:LYREL_CAPTURE_LABEL='explicit-empty-fields'; omp {observer}\r"));
        lines.push(call("after-explicit-empty-fields", "ordinary"));
        lines.push(format!("$env:TERM_PROGRAM=$null; $env:KITTY_WINDOW_ID=$null; $env:LYREL_CAPTURE_LABEL='explicit-unset'; hermes {observer}\r"));
        lines.push(call("after-explicit-unset", "ordinary"));
    }
    lines.push(call("failure", "omp"));
    lines.push(call("after-failure", "ordinary"));
    lines.push(call("interrupt", "omp"));
    lines.push("\u{3}".into());
    lines.push(call("after-interrupt", "ordinary"));
    if !nushell {
        let native = bin.join(if cfg!(windows) { "omp.exe" } else { "omp" });
        lines.push(call("absolute", &format!("& '{}'", native.to_string_lossy().replace('\'', "''"))));
        lines.push(format!("$env:LYREL_CAPTURE_LABEL='pipeline'; omp {observer} | Out-Null\r"));
        lines.push(format!("$env:LYREL_CAPTURE_LABEL='compound'; omp {observer}; $null=0\r"));
        // PS5.1 cannot parse the trailing background operator. Exercise its
        // public job mechanism with the same production explicit-scope encoder.
        let background = crate::host::session_operations::scoped_agent_launch_command(
            &*crate::host::local::LocalHost::new(),
            crate::core::cli_agent::CLIAgent::OhMyPi,
            &format!("omp {observer}"),
            Some(program),
        ).unwrap();
        let script = home.path().join("background.ps1");
        std::fs::write(&script, format!("$env:LYREL_CAPTURE_LABEL='background-job'\n{background}\n$env:LYREL_CAPTURE_LABEL='after-job-scope'\nordinary {observer}\n")).unwrap();
        lines.push(format!("Start-Job -ScriptBlock {{ & '{}' }}\r", script.to_string_lossy().replace('\'', "''")));
        lines.push("Get-Job | Wait-Job | Receive-Job; Get-Job | Remove-Job\r".into());
        lines.push(call("after-background-job", "ordinary"));
        if powershell_major >= 6 {
            lines.push(format!("$env:LYREL_CAPTURE_LABEL='background'; omp {observer} &\r"));
            lines.push("Get-Job | Wait-Job | Receive-Job; Get-Job | Remove-Job\r".into());
            lines.push(call("after-background", "ordinary"));
        }
    }
    lines.push(if nushell { "1 / 0\r".into() } else { "Write-Error fixture\r".into() });
    let steps: Vec<_> = lines.iter().map(|line| line.as_bytes()).collect();
    let text = prompt_steps_over_pty(program, &injection, &steps, Some(home.path()));
    for agent in ["omp", "kimi", "pi", "opencode", "opencode2", "hermes"] {
        let read = |label: &str| -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap_or_else(|error| panic!("{label}: {error}\n{text}"))).unwrap()
        };
        let actual = read(agent);
        assert_eq!(actual["TERM_PROGRAM"], "kitty", "{program}/{agent}: {text}");
        assert_eq!(actual["TERM_PROGRAM_VERSION"], "");
        assert_eq!(actual["LC_TERMINAL"], "lyrel");
        assert_eq!(actual["LC_TERMINAL_VERSION"], env!("CARGO_PKG_VERSION"));
        assert_eq!(actual["PI_NO_DECCARA"], if agent == "omp" { serde_json::json!("1") } else { serde_json::Value::Null });
        assert_eq!(actual["KITTY_WINDOW_ID"], if agent == "hermes" { serde_json::json!("lyrel") } else { serde_json::Value::Null });
        assert_eq!(actual["PI_IMAGE_PROTOCOL"], "none");
        assert_eq!(actual["PI_FORCE_IMAGE_PROTOCOL"], "off");
        assert_eq!(actual["PI_NO_KITTY_PLACEHOLDERS"], "1");
        assert_eq!(actual["OPENCODE_CLI_CONFIG_CONTENT"], "{\"session\":{\"image_preview\":false}}");
        let restored = read(&format!("after-{agent}"));
        assert_eq!(restored["TERM_PROGRAM"], "lyrel");
        assert_eq!(restored["TERM_PROGRAM_VERSION"], "original-version");
        assert_eq!(restored["LC_TERMINAL"], "");
        assert_eq!(restored["LC_TERMINAL_VERSION"], serde_json::Value::Null);
        assert_eq!(restored["PI_NO_DECCARA"], serde_json::Value::Null);
        assert_eq!(restored["KITTY_WINDOW_ID"], serde_json::Value::Null);
    }
    for label in ["after-failure", "after-interrupt"] {
        let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap()).unwrap();
        assert_eq!(actual["TERM_PROGRAM"], "lyrel", "{program}/{label}: {text}");
    }
    let explicit: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join("explicit.json")).unwrap()).unwrap();
    assert_eq!(explicit["TERM_PROGRAM"], "explicit");
    assert_eq!(explicit["TERM_PROGRAM_VERSION"], "chosen");
    for (label, key) in [("empty-identity", "TERM_PROGRAM"), ("empty-marker", "KITTY_WINDOW_ID")] {
        let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap()).unwrap();
        assert_eq!(actual[key], "", "{program}/{label}: {text}");
        let restored: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("after-{label}.json"))).unwrap()).unwrap();
        assert_eq!(restored[key], if key == "TERM_PROGRAM" { serde_json::json!("lyrel") } else { serde_json::Value::Null }, "{program}/after-{label}: {text}");
    }
    if !nushell {
        let read = |label: &str| -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap_or_else(|error| panic!("{label}: {error}\n{text}"))).unwrap()
        };
        let empty = read("explicit-empty-fields");
        for key in ["TERM_PROGRAM_VERSION", "LC_TERMINAL", "LC_TERMINAL_VERSION", "PI_NO_DECCARA", "OPENCODE_CLI_CONFIG_CONTENT"] {
            assert_eq!(empty[key], "", "{program}/explicit-empty-fields/{key}: {text}");
        }
        let restored = read("after-explicit-empty-fields");
        assert_eq!(restored["TERM_PROGRAM_VERSION"], "original-version");
        assert_eq!(restored["LC_TERMINAL"], "");
        assert_eq!(restored["LC_TERMINAL_VERSION"], serde_json::Value::Null);
        assert_eq!(restored["PI_NO_DECCARA"], serde_json::Value::Null);
        assert_eq!(restored["OPENCODE_CLI_CONFIG_CONTENT"], "{\"session\":{\"image_preview\":false}}");
        let unset = read("explicit-unset");
        for key in ["TERM_PROGRAM", "KITTY_WINDOW_ID"] {
            assert_eq!(unset[key], serde_json::Value::Null, "{program}/explicit-unset/{key}: {text}");
        }
        let background = read("background-job");
        assert_eq!(background["TERM_PROGRAM"], "kitty", "{program}/background-job: {text}");
        assert_eq!(background["TERM_PROGRAM_VERSION"], "");
        assert_eq!(background["PI_NO_DECCARA"], "1");
        for key in ["PI_IMAGE_PROTOCOL", "PI_FORCE_IMAGE_PROTOCOL", "PI_KITTY_PLACEHOLDERS", "PI_NO_KITTY_PLACEHOLDERS", "OPENCODE_CLI_CONFIG_CONTENT"] {
            assert_eq!(background[key], read("after-job-scope")[key], "{program}/background-job/{key}: {text}");
        }
        if powershell_major >= 6 {
            let background = read("background");
            assert_eq!(background["TERM_PROGRAM"], "kitty", "{program}/background: {text}");
            assert_eq!(background["TERM_PROGRAM_VERSION"], "");
            assert_eq!(read("after-background")["TERM_PROGRAM"], "lyrel");
        }
        for label in ["pipeline", "compound", "after-job-scope", "after-background-job", "after-explicit-unset"] {
            let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap()).unwrap();
            assert_eq!(actual["TERM_PROGRAM"], "lyrel", "{program}/{label}: {text}");
        }

        // Inherit empty values from the native launcher, including on Windows
        // PowerShell versions whose own env assignments delete empty strings.
        let fields = [
            ("TERM_PROGRAM", "original-terminal"),
            ("TERM_PROGRAM_VERSION", "original-version"),
            ("LC_TERMINAL", "original-locale"),
            ("LC_TERMINAL_VERSION", "original-locale-version"),
            ("PI_NO_DECCARA", "0"),
            ("KITTY_WINDOW_ID", "owned-marker"),
            ("OPENCODE_CLI_CONFIG_CONTENT", "{\"session\":{\"image_preview\":false}}"),
        ];
        for (key, _) in fields { injection.env.insert(key.into(), String::new()); }
        let mut lines = Vec::new();
        for state in ["empty", "unset", "nonempty"] {
            if state == "unset" {
                lines.push(format!("Remove-Item {} -ErrorAction Ignore\r", fields.iter().map(|(key, _)| format!("Env:{key}")).collect::<Vec<_>>().join(",")));
            } else if state == "nonempty" {
                lines.push(format!("{}\r", fields.iter().map(|(key, value)| format!("$env:{key}='{}'", value.replace('\'', "''"))).collect::<Vec<_>>().join("; ")));
            }
            lines.push(call(&format!("roundtrip-{state}-before"), "ordinary"));
            for agent in ["omp", "hermes"] {
                lines.push(call(&format!("roundtrip-{state}-{agent}"), agent));
                lines.push(call(&format!("roundtrip-{state}-after-{agent}"), "ordinary"));
            }
        }
        lines.push("Write-Error fixture\r".into());
        let steps: Vec<_> = lines.iter().map(|line| line.as_bytes()).collect();
        let text = prompt_steps_over_pty(program, &injection, &steps, Some(home.path()));
        for state in ["empty", "unset", "nonempty"] {
            for suffix in ["before", "after-omp", "after-hermes"] {
                let label = format!("roundtrip-{state}-{suffix}");
                let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("{label}.json"))).unwrap_or_else(|error| panic!("{label}: {error}\n{text}"))).unwrap();
                for (key, value) in fields {
                    let expected = match state {
                        "empty" => serde_json::json!(""),
                        "unset" => serde_json::Value::Null,
                        _ => serde_json::json!(value),
                    };
                    assert_eq!(actual[key], expected, "{program}/{label}/{key}: {text}");
                }
                for (key, value) in [("PI_IMAGE_PROTOCOL", "none"), ("PI_FORCE_IMAGE_PROTOCOL", "off"), ("PI_KITTY_PLACEHOLDERS", "0"), ("PI_NO_KITTY_PLACEHOLDERS", "1")] {
                    assert_eq!(actual[key], value, "{program}/{label}/{key}: {text}");
                }
            }
        }
    }
    if let Some(dir) = injection.dir { std::fs::remove_dir_all(dir).unwrap(); }
}

#[test]
fn powershell_manual_agent_scope_and_restoration_over_a_real_pty() {
    structured_scope_shell_contract(&std::env::var("LYREL_TEST_PWSH").unwrap_or_else(|_| "pwsh".into()), false);
}

#[test]
fn nushell_manual_agent_wrappers_and_restoration_over_a_real_pty() {
    structured_scope_shell_contract(&std::env::var("LYREL_TEST_NU").unwrap_or_else(|_| "nu".into()), true);
}

/// The four shell hooks cannot share Rust constants at runtime, so pin their
/// agent list to the production one and keep Nu's omp default identical to the
/// other three hooks (an inherited value is not an explicit opt-out).
#[test]
fn every_shell_scope_script_covers_the_production_agent_programs() {
    let programs: Vec<_> = crate::core::cli_agent::CLIAgent::scope_programs().collect();
    for (name, script) in [
        ("sh", include_str!("agent_scope.sh")),
        ("fish", include_str!("agent_scope.fish")),
        ("ps1", include_str!("agent_scope.ps1")),
        ("nu", include_str!("agent_scope.nu")),
    ] {
        for program in &programs {
            assert!(script.contains(program), "{name} is missing {program}");
        }
    }
    assert!(
        include_str!("agent_scope.nu").contains("insert PI_NO_DECCARA '1'"),
        "Nu must default PI_NO_DECCARA to 1 for omp like the other hooks"
    );
}
