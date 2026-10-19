use super::*;

const ENV: &[&str] = &["HOME", "USERPROFILE", "KIMI_CODE_HOME"];

pub(super) fn prepare<H: Host + ?Sized>(
    host: &H,
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    let mut scope = request.scope.clone();
    if scope
        .environment
        .keys()
        .any(|key| !ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Kimi scope accepts only HOME, USERPROFILE and KIMI_CODE_HOME",
        ));
    }
    if scope.session_dir.is_some() {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Kimi storage is selected by KIMI_CODE_HOME, not session-dir",
        ));
    }
    let mut argv = request.launch_argv.clone();
    while let Some((key, value)) = argv.first().and_then(|arg| arg.split_once('=')) {
        if !ENV.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "unsupported Kimi environment assignment",
            ));
        }
        scope
            .environment
            .entry(key.into())
            .or_insert_with(|| value.into());
        argv.remove(0);
    }
    if argv.is_empty() {
        argv.push("kimi".into());
    }
    let named = (0..argv.len())
        .find(|i| CLIAgent::detect_from_argv(&argv[..=*i]) == Some(CLIAgent::Kimi))
        .or_else(|| {
            (argv.len() >= 2
                && matches!(
                    Path::new(&argv[0])
                        .file_stem()
                        .and_then(|stem| stem.to_str()),
                    Some("node" | "bun")
                )
                && argv[1]
                    .replace('\\', "/")
                    .ends_with("/@moonshot-ai/kimi-code/dist/main.mjs"))
            .then_some(1)
        })
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Unsupported,
                "cannot identify the Kimi executable in captured launch context",
            )
        })?;
    let program = argv[..=named].to_vec();
    let mut launch = program.clone();
    let mut index = named + 1;
    let values = ["--model", "-m", "--skills-dir", "--add-dir"];
    let switches = [
        "--yolo",
        "-y",
        "--auto",
        "--yes",
        "--auto-approve",
        "--plan",
    ];
    while index < argv.len() {
        let arg = &argv[index];
        let key = arg.split('=').next().unwrap_or(arg);
        if matches!(
            key,
            "--session"
                | "-S"
                | "--resume"
                | "-r"
                | "--agent"
                | "--agent-file"
                | "--cwd"
                | "--prompt"
                | "-p"
                | "--ui"
                | "--output-format"
        ) {
            index += if arg.contains('=') { 1 } else { 2 };
            continue;
        }
        if matches!(
            key,
            "--continue" | "-c" | "-C" | "--print" | "--acp" | "--wire"
        ) {
            index += 1;
            continue;
        }
        if !values.contains(&key) && !switches.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "Kimi launch contains an option whose interactive replay semantics are not supported",
            ));
        }
        launch.push(arg.clone());
        if values.contains(&key) && !arg.contains('=') {
            index += 1;
            launch.push(
                argv.get(index)
                    .ok_or_else(|| {
                        failure(SessionErrorKind::Unsupported, "incomplete Kimi option")
                    })?
                    .clone(),
            );
        }
        index += 1;
    }
    let defaults = host
        .agent_environment()
        .map_err(|error| failure(SessionErrorKind::Directory, error.to_string()))?;
    for key in ENV {
        if let Some(value) = defaults.get(*key) {
            scope
                .environment
                .entry((*key).into())
                .or_insert_with(|| value.clone());
        }
    }
    let home = match scope.environment.get("KIMI_CODE_HOME") {
        Some(home) => resolve(&request.cwd, Path::new(home)),
        None => {
            if let Some(home) = scope
                .environment
                .get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .filter(|value| !value.is_empty())
            {
                resolve(&request.cwd, Path::new(home)).join(".kimi-code")
            } else {
                return Err(failure(
                    SessionErrorKind::Directory,
                    "this Host reports no Kimi home selector",
                ));
            }
        }
    };
    scope
        .environment
        .insert("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned());
    let (source_file, source) = locate(host, &home, &request.source_id)?;
    if let Some(selected) = &scope.session_file {
        let selected = host
            .canonicalize(selected)
            .map_err(|error| failure(SessionErrorKind::Identity, error.to_string()))?;
        let actual = host
            .canonicalize(&source_file)
            .map_err(|error| failure(SessionErrorKind::Identity, error.to_string()))?;
        if selected != actual {
            return Err(failure(
                SessionErrorKind::Identity,
                "Kimi source does not match the selected storage scope",
            ));
        }
    }
    validate_cwd(host, &source, &request.cwd)?;
    command_line(&launch, &scope)?;
    let (file, id, created) = if request.kind == SessionOperationKind::Fork {
        let version = output(&program, &["--version"], &scope, &request.cwd)?;
        if version != "2.2.0\n" && version != "2.2.0\r\n" {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "Kimi native fork stdout contract is verified only for official 2.2.0",
            ));
        }
        let stdout = output(
            &program,
            &["fork", &request.source_id],
            &scope,
            &request.cwd,
        )
        .map_err(|mut error| {
            error.message.push_str(
                "; native creation may already have succeeded; inspect history before repeating",
            );
            error
        })?;
        let id = fork_id(&stdout).ok_or_else(|| failure(SessionErrorKind::Identity,
            "Kimi 2.2.0 fork stdout contract changed; native creation may already have succeeded; inspect history before repeating"))?;
        if id == request.source_id {
            return Err(failure(
                SessionErrorKind::Identity,
                "Kimi fork returned the original identity",
            ));
        }
        let retained = |message: String| {
            let prepared = PreparedSession {
                host_id: request.expected_host,
                agent: request.agent,
                source_id: request.source_id.clone(),
                target_id: Some(id.clone()),
                session_file: None,
                cwd: request.cwd.clone(),
                scope: scope.clone(),
                launch_argv: launch.clone(),
                command: None,
                created: true,
            };
            let mut error = prepared.opening_failed(message);
            error.kind = SessionErrorKind::Identity;
            error
        };
        let (file, target) =
            locate(host, &home, &id).map_err(|error| retained(error.to_string()))?;
        validate_cwd(host, &target, &request.cwd).map_err(|error| retained(error.to_string()))?;
        if target["forkedFrom"].as_str() != Some(request.source_id.as_str())
            || !stdout_matches(&stdout, &id, target["title"].as_str())
        {
            return Err(retained(
                "Kimi fork result does not match the verified native target and stdout contract"
                    .into(),
            ));
        }
        (file, id, true)
    } else {
        (source_file, request.source_id.clone(), false)
    };
    scope.session_file = Some(file.clone());
    let mut open = launch.clone();
    open.extend(["--session".into(), id.clone()]);
    let command = command_line(&open, &scope);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: request.agent,
        source_id: request.source_id.clone(),
        target_id: Some(id),
        session_file: Some(file),
        cwd: request.cwd.clone(),
        scope,
        launch_argv: launch,
        command: command.as_ref().ok().cloned(),
        created,
    };
    match command {
        Ok(_) => Ok(prepared),
        Err(error) => Err(prepared.opening_failed(error.to_string())),
    }
}

fn locate<H: Host + ?Sized>(
    host: &H,
    home: &Path,
    id: &str,
) -> Result<(PathBuf, serde_json::Value), SessionOperationError> {
    let root = host.join(home, "sessions");
    let projects = host.read_dir(&root, None).map_err(|error| {
        failure(
            SessionErrorKind::Identity,
            format!("cannot read selected Kimi history: {error}"),
        )
    })?;
    let mut found = None;
    for project in projects
        .into_iter()
        .filter(|entry| entry.is_dir && !entry.is_symlink)
    {
        let file = host.join(
            &host.join(&host.join(&root, &project.name), id),
            "state.json",
        );
        let bytes = match host.read_file(&file, 256 * 1024) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(failure(
                    SessionErrorKind::Identity,
                    format!("cannot read Kimi source: {error}"),
                ));
            }
        };
        let state: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            failure(
                SessionErrorKind::Identity,
                format!("invalid Kimi metadata: {error}"),
            )
        })?;
        if state["id"].as_str() != Some(id) || state["archived"] == true {
            return Err(failure(
                SessionErrorKind::Identity,
                "Kimi target is missing, archived, or has a mismatched identity",
            ));
        }
        if found.is_some() {
            return Err(failure(
                SessionErrorKind::Identity,
                "Kimi identity is ambiguous within the selected home",
            ));
        }
        found = Some((file, state));
    }
    found.ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "selected Kimi session no longer exists in this storage scope",
        )
    })
}

fn validate_cwd<H: Host + ?Sized>(
    host: &H,
    state: &serde_json::Value,
    cwd: &Path,
) -> Result<(), SessionOperationError> {
    let stored = state["cwd"].as_str().ok_or_else(|| {
        failure(
            SessionErrorKind::Directory,
            "Kimi metadata has no working directory",
        )
    })?;
    let stored = host.canonicalize(Path::new(stored)).map_err(|error| {
        failure(
            SessionErrorKind::Directory,
            format!("Kimi working directory is unavailable: {error}"),
        )
    })?;
    let requested = host
        .canonicalize(cwd)
        .map_err(|error| failure(SessionErrorKind::Directory, error.to_string()))?;
    if stored != requested {
        return Err(failure(
            SessionErrorKind::Identity,
            "Kimi working directory differs from the selected session",
        ));
    }
    Ok(())
}

fn fork_id(stdout: &str) -> Option<String> {
    let rest = stdout.strip_prefix("Forked to ")?;
    let id = rest.split_once(' ')?.0;
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte)))
    .then(|| id.into())
}

fn stdout_matches(stdout: &str, id: &str, title: Option<&str>) -> bool {
    let title = title
        .map(|title| format!(" (\"{title}\")"))
        .unwrap_or_default();
    let prefix = format!("Forked to {id}{title} in ");
    stdout
        .strip_prefix(&prefix)
        .and_then(|rest| {
            rest.strip_suffix("ms\n")
                .or_else(|| rest.strip_suffix("ms\r\n"))
        })
        .is_some_and(|elapsed| {
            !elapsed.is_empty() && elapsed.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn output(
    program: &[String],
    args: &[&str],
    scope: &SessionScope,
    cwd: &Path,
) -> Result<String, SessionOperationError> {
    let mut command = Command::new(&program[0]);
    command
        .args(&program[1..])
        .args(args)
        .envs(&scope.environment)
        .current_dir(cwd)
        .env_remove("LYREL")
        .env_remove(crate::daemon::pane::LYREL_PANE_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::core::proc::hide_console(&mut command);
    let mut child = command.spawn().map_err(|error| {
        failure(
            SessionErrorKind::Command,
            format!("Kimi session operation: {error}"),
        )
    })?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stream
                .take(1024 * 1024)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        })
    };
    let stdout = read(Box::new(stdout));
    let stderr = read(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failure(
                    SessionErrorKind::Command,
                    match result {
                        Err(error) => error.to_string(),
                        _ => "Kimi session operation timed out".into(),
                    },
                ));
            }
        }
    };
    let stdout = stdout
        .join()
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| failure(SessionErrorKind::Command, "cannot read Kimi stdout"))?;
    let stderr = stderr
        .join()
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| failure(SessionErrorKind::Command, "cannot read Kimi stderr"))?;
    if !status.success() {
        return Err(failure(
            SessionErrorKind::Command,
            format!(
                "Kimi session operation failed: {}",
                String::from_utf8_lossy(&stderr)
                    .chars()
                    .take(1500)
                    .collect::<String>()
            ),
        ));
    }
    String::from_utf8(stdout)
        .map_err(|_| failure(SessionErrorKind::Command, "Kimi returned non-UTF-8 stdout"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::local::LocalHost;

    fn request(dir: &Path, root: &Path) -> SessionOperation {
        let mut scope = SessionScope::default();
        scope
            .environment
            .insert("KIMI_CODE_HOME".into(), root.to_string_lossy().into_owned());
        SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Kimi,
            kind: SessionOperationKind::Resume,
            source_id: "session-original".into(),
            cwd: dir.into(),
            launch_argv: vec!["kimi".into(), "--model".into(), "team/model".into()],
            scope,
        }
    }

    #[test]
    fn kimi_missing_history_never_prepares_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path(), dir.path());
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Identity);
        assert!(error.retained.is_none());
    }

    #[test]
    fn kimi_resume_pins_the_selected_home_and_preserves_native_context() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("隔离 home");
        let session = home.join("sessions/wd_project_123/session-original");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("state.json"),
            serde_json::json!({
                "id":"session-original", "cwd":dir.path(), "title":"Original", "archived":false,
                "agents":{"main":{"profileName":"reviewer"}}, "updatedAt":1792320000000u64
            })
            .to_string(),
        )
        .unwrap();
        let mut request = request(dir.path(), &home);
        request
            .launch_argv
            .extend(["--continue".into(), "--agent".into(), "reviewer".into()]);
        let prepared = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(prepared.target_id.as_deref(), Some("session-original"));
        assert_eq!(
            prepared.scope.environment["KIMI_CODE_HOME"],
            home.to_string_lossy()
        );
        assert_eq!(
            prepared.session_file.as_deref(),
            Some(session.join("state.json").as_path())
        );
        assert!(
            prepared
                .launch_argv
                .windows(2)
                .any(|v| v == ["--model", "team/model"])
        );
        assert!(
            !prepared
                .launch_argv
                .iter()
                .any(|v| matches!(v.as_str(), "--continue" | "--agent"))
        );
        assert!(
            prepared
                .command
                .unwrap()
                .contains("--session session-original")
        );
        assert!(!prepared.created);
    }

    #[test]
    fn kimi_explicit_empty_home_is_the_native_cwd_not_default_history() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("sessions/project/session-original");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("state.json"),
            serde_json::json!({"id":"session-original","cwd":dir.path()}).to_string(),
        )
        .unwrap();
        let mut request = request(dir.path(), dir.path());
        request
            .scope
            .environment
            .insert("KIMI_CODE_HOME".into(), String::new());
        let prepared = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(prepared.target_id.as_deref(), Some("session-original"));
    }

    #[test]
    fn kimi_persisted_launch_uses_the_hook_reported_home_without_replaying_profile_as_a_flag() {
        let event = crate::core::cli_agent::parse_agent_event_body(br#"{"agent":"kimi","event":"session-start","session_id":"s","session_scope":{"profile":"reviewer","environment":{"KIMI_CODE_HOME":"/actual/home"}}}"#).unwrap();
        let mut state = crate::core::cli_agent::AgentSessionState::default();
        state.launch_argv = Some(vec![
            "KIMI_CODE_HOME=/old/home".into(),
            "kimi".into(),
            "--model".into(),
            "team/model".into(),
        ]);
        state.apply_event(&event);
        let launch = launch_context(CLIAgent::Kimi, &state).unwrap();
        assert_eq!(
            launch,
            [
                "KIMI_CODE_HOME=/actual/home",
                "kimi",
                "--model",
                "team/model"
            ]
        );
    }

    #[test]
    fn kimi_source_from_another_scope_is_rejected_before_launch() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path(), &dir.path().join("selected"));
        let file = dir.path().join("other/state.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            serde_json::json!({"id":"session-original", "cwd":dir.path()}).to_string(),
        )
        .unwrap();
        request.scope.session_file = Some(file);
        assert_eq!(
            LocalHost::new()
                .session_operation(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Identity
        );
    }

    #[test]
    #[ignore = "requires isolated official LYREL_TEST_KIMI, LYREL_TEST_KIMI_HOME, LYREL_TEST_KIMI_SESSION and LYREL_TEST_KIMI_CWD"]
    fn native_kimi_fork_retains_an_independent_reopenable_identity() {
        let cwd = PathBuf::from(std::env::var("LYREL_TEST_KIMI_CWD").unwrap());
        let home = PathBuf::from(std::env::var("LYREL_TEST_KIMI_HOME").unwrap());
        let mut request = request(&cwd, &home);
        request.launch_argv =
            crate::core::cli_agent::parse_command_argv(&std::env::var("LYREL_TEST_KIMI").unwrap())
                .unwrap();
        request.source_id = std::env::var("LYREL_TEST_KIMI_SESSION").unwrap();
        request.kind = SessionOperationKind::Fork;
        let host = LocalHost::new();
        let prepared = host.session_operation(&request).unwrap();
        assert!(prepared.created);
        assert_ne!(
            prepared.target_id.as_deref(),
            Some(request.source_id.as_str())
        );
        let retained = prepared
            .clone()
            .opening_failed("new pane failed")
            .retained
            .unwrap();
        assert_eq!(retained.target_id, prepared.target_id);
        request.kind = SessionOperationKind::Resume;
        let original = host.session_operation(&request).unwrap();
        assert_eq!(
            original.target_id.as_deref(),
            Some(request.source_id.as_str())
        );
        request.source_id = prepared.target_id.unwrap();
        request.scope = prepared.scope;
        let reopened = host.session_operation(&request).unwrap();
        assert_eq!(
            reopened.target_id.as_deref(),
            Some(request.source_id.as_str())
        );
        assert!(!reopened.created);
    }
}
