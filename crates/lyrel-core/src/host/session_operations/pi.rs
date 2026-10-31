use super::*;

const ENV: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "PI_CODING_AGENT_DIR",
    "PI_CODING_AGENT_SESSION_DIR",
    "PI_IMAGE_PROTOCOL",
];

pub(super) fn prepare<H: Host + ?Sized>(
    host: &H,
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    let mut scope = request.scope.clone();
    if scope.profile.is_some()
        || scope.opencode.is_some()
        || scope
            .environment
            .keys()
            .any(|key| !ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Pi has no omp profile or OpenCode connection selector; only Pi home, storage and image protocol selectors can be replayed",
        ));
    }
    let mut argv = request.launch_argv.as_slice();
    while let Some((key, value)) = argv.first().and_then(|arg| arg.split_once('=')) {
        if !ENV.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "unsupported Pi environment selector",
            ));
        }
        if !request.scope.environment.contains_key(key) {
            scope.environment.insert(key.into(), value.into());
        }
        argv = &argv[1..];
    }
    let fallback = ["pi".to_string()];
    if argv.is_empty() {
        argv = &fallback;
    }
    let named = (0..argv.len())
        .find(|i| CLIAgent::detect_from_argv(&argv[..=*i]) == Some(CLIAgent::Pi))
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Unsupported,
                "cannot identify the Pi executable in the captured launch context",
            )
        })?;
    let mut launch = argv[..=named].to_vec();
    let values = crate::core::cli_agent::PI_VALUE_OPTIONS;
    let switches = crate::core::cli_agent::PI_SWITCHES;
    let mut index = named + 1;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            break;
        }
        let key = arg.split('=').next().unwrap_or(arg);
        if key == "--api-key" {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "credential arguments cannot be replayed; configure credentials on the Pi host",
            ));
        }
        if key == "--no-session" {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "Pi session persistence is disabled",
            ));
        }
        let takes_value = values.contains(&key)
            || matches!(
                key,
                "--session" | "--session-id" | "--fork" | "--session-dir" | "--mode" | "--export"
            );
        let value = if takes_value {
            Some(if let Some((_, value)) = arg.split_once('=') {
                value
            } else {
                index += 1;
                argv.get(index)
                    .ok_or_else(|| {
                        failure(SessionErrorKind::Unsupported, "incomplete Pi launch option")
                    })?
                    .as_str()
            })
        } else {
            None
        };
        match key {
            "--session" => {
                if scope.session_file.is_none() {
                    if let Some(value) = value
                        .filter(|value| value.ends_with(".jsonl") || value.contains(['/', '\\']))
                    {
                        scope.session_file = Some(resolve(&request.cwd, Path::new(value)));
                    }
                }
            }
            "--session-dir" => {
                if scope.session_dir.is_none() {
                    scope.session_dir = value.map(|value| resolve(&request.cwd, Path::new(value)));
                }
            }
            "--session-id" | "--fork" | "--continue" | "-c" | "--resume" | "-r" | "--mode"
            | "--print" | "-p" | "--export" | "--help" | "-h" | "--version" | "-v"
            | "--list-models" => {}
            _ if values.contains(&key) || switches.contains(&key) => {
                // Native built-in options do not accept --key=value. Normalize
                // captured spellings before invoking the published CLI parser.
                launch.push(key.into());
                if let Some(value) = value {
                    launch.push(value.into());
                }
            }
            _ if arg.starts_with("--") => {
                // Pi extensions own arbitrary long options. Match Pi's public
                // parser (a following non-option token is the flag value).
                launch.push(arg.clone());
                if !arg.contains('=')
                    && argv
                        .get(index + 1)
                        .is_some_and(|next| !next.starts_with(['-', '@']))
                {
                    index += 1;
                    launch.push(argv[index].clone());
                }
            }
            _ if arg.starts_with('-') => {
                return Err(failure(
                    SessionErrorKind::Unsupported,
                    "unknown Pi short option cannot be replayed safely",
                ));
            }
            _ => {} // Initial prompts and @file inputs belong to the old run.
        }
        index += 1;
    }
    let environment = host.agent_environment().map_err(|e| {
        failure(
            SessionErrorKind::Unsupported,
            format!("Pi host scope is unavailable: {e}"),
        )
    })?;
    for (key, value) in environment
        .into_iter()
        .filter(|(key, _)| ENV.contains(&key.as_str()))
    {
        scope.environment.entry(key).or_insert(value);
    }
    let select = |key: &str| scope.environment.get(key).filter(|value| !value.is_empty());
    let home = select(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .or_else(|| select("HOME"))
        .or_else(|| select("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Directory,
                "Pi host has no absolute effective home",
            )
        })?;
    let agent_dir = select("PI_CODING_AGENT_DIR")
        .map(|value| expand(value, &home, &request.cwd))
        .unwrap_or_else(|| home.join(".pi/agent"));
    if scope.session_dir.is_none() {
        scope.session_dir = crate::core::agent_history::pi_session_dir(
            &agent_dir,
            &home,
            Some(&request.cwd),
            scope
                .environment
                .get("PI_CODING_AGENT_SESSION_DIR")
                .map(String::as_str),
        );
    }
    let source = match &scope.session_file {
        Some(file) => resolve(&request.cwd, file),
        None => locate(
            host,
            scope
                .session_dir
                .as_deref()
                .unwrap_or(&agent_dir.join("sessions")),
            &request.source_id,
        )?,
    };
    let source_id = header(&source)?;
    if source_id != request.source_id {
        return Err(failure(
            SessionErrorKind::Identity,
            "Pi source file does not match the selected session identity",
        ));
    }
    if let Some(dir) = &scope.session_dir {
        let root = std::fs::canonicalize(dir).map_err(|e| {
            failure(
                SessionErrorKind::Directory,
                format!("Pi session directory: {e}"),
            )
        })?;
        let file = std::fs::canonicalize(&source)
            .map_err(|e| failure(SessionErrorKind::Identity, e.to_string()))?;
        if !file.starts_with(root) {
            return Err(failure(
                SessionErrorKind::Identity,
                "Pi source is outside the selected storage scope",
            ));
        }
    } else {
        scope.session_dir = source.parent().map(Path::to_path_buf);
    }
    if let Some(dir) = &scope.session_dir {
        launch.extend(["--session-dir".into(), dir.to_string_lossy().into_owned()]);
    }
    command_line(&launch, &scope)?;
    let (file, target, shutdown_error) = if request.kind == SessionOperationKind::Fork {
        native_fork(&launch, &scope, request, &source).map_err(|mut error| {
            error.message = format!(
                "{}; Pi may already have created the fork — inspect native history before retrying",
                error.message
            );
            error
        })?
    } else {
        (source, source_id, None)
    };
    scope.session_file = Some(file.clone());
    let mut open = launch.clone();
    open.extend(["--session".into(), file.to_string_lossy().into_owned()]);
    let command = command_line(&open, &scope);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: CLIAgent::Pi,
        source_id: request.source_id.clone(),
        target_id: Some(target),
        session_file: Some(file),
        cwd: request.cwd.clone(),
        scope,
        launch_argv: launch,
        command: command.as_ref().ok().cloned(),
        created: request.kind == SessionOperationKind::Fork,
    };
    if let Err(error) = command {
        return Err(prepared.opening_failed(error.to_string()));
    }
    if let Some(error) = shutdown_error {
        let mut error = prepared.opening_failed(error);
        error.kind = SessionErrorKind::Command;
        return Err(error);
    }
    Ok(prepared)
}

fn expand(value: &str, home: &Path, cwd: &Path) -> PathBuf {
    if value == "~" {
        home.into()
    } else if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        home.join(rest)
    } else {
        resolve(cwd, Path::new(value))
    }
}

fn header(file: &Path) -> Result<String, SessionOperationError> {
    let file = std::fs::File::open(file).map_err(|e| {
        failure(
            SessionErrorKind::Identity,
            format!("cannot open Pi source: {e}"),
        )
    })?;
    let mut line = String::new();
    io::BufReader::new(file)
        .take(256 * 1024)
        .read_line(&mut line)
        .map_err(|e| failure(SessionErrorKind::Identity, e.to_string()))?;
    let value: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
        failure(
            SessionErrorKind::Identity,
            format!("invalid Pi session header: {e}"),
        )
    })?;
    value["id"]
        .as_str()
        .filter(|id| value["type"] == "session" && !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "Pi source has no persisted identity",
            )
        })
}

fn locate<H: Host + ?Sized>(
    host: &H,
    root: &Path,
    id: &str,
) -> Result<PathBuf, SessionOperationError> {
    let entries = host
        .read_dir(root, None)
        .map_err(|e| failure(SessionErrorKind::Directory, format!("Pi storage: {e}")))?;
    let mut found = None;
    for entry in entries {
        let path = host.join(root, &entry.name);
        let files = if entry.is_dir {
            host.read_dir(&path, None)
                .map_err(|e| failure(SessionErrorKind::Directory, e.to_string()))?
                .into_iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| host.join(&path, &entry.name))
                .collect()
        } else {
            vec![path]
        };
        for file in files {
            if file.extension().is_some_and(|ext| ext == "jsonl")
                && header(&file).is_ok_and(|candidate| candidate == id)
            {
                if found.is_some() {
                    return Err(failure(
                        SessionErrorKind::Identity,
                        "Pi session ID is ambiguous in this storage scope; select its exact history entry",
                    ));
                }
                found = Some(file);
            }
        }
    }
    found.ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "Pi session does not exist in the selected storage scope",
        )
    })
}

fn native_fork(
    launch: &[String],
    scope: &SessionScope,
    request: &SessionOperation,
    source: &Path,
) -> Result<(PathBuf, String, Option<String>), SessionOperationError> {
    let cwd = request.cwd.as_path();
    let mut command = Command::new(&launch[0]);
    command
        .args(&launch[1..])
        .arg("--fork")
        .arg(source)
        .args(["--mode", "rpc"])
        .current_dir(cwd)
        .envs(&scope.environment)
        .env_remove("LYREL")
        .env_remove(crate::daemon::pane::LYREL_PANE_ENV);
    crate::core::proc::hide_console(&mut command);
    let (output, timed_out) = rpc_output(&mut command)
        .map_err(|e| failure(SessionErrorKind::Command, format!("Pi native fork: {e}")))?;
    let state = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find(|value| {
            value["type"] == "response"
                && value["command"] == "get_state"
                && value["id"] == "lyrel-session-state"
        })
        .ok_or_else(|| {
            let detail = String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1500)
                .collect::<String>();
            failure(
                SessionErrorKind::Command,
                format!(
                    "Pi native fork {} ({}): {detail}",
                    if timed_out {
                        "timed out before reporting identity"
                    } else {
                        "did not report a get_state identity"
                    },
                    output.status
                ),
            )
        })?;
    if state["success"] != true {
        return Err(failure(
            SessionErrorKind::Command,
            state["error"]
                .as_str()
                .unwrap_or("Pi native get_state failed"),
        ));
    }
    let id = state["data"]["sessionId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "Pi native fork returned no session ID",
            )
        })?
        .to_owned();
    if id == request.source_id {
        return Err(failure(
            SessionErrorKind::Identity,
            "Pi did not create an independent fork identity",
        ));
    }
    let path = state["data"]["sessionFile"]
        .as_str()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            let mut target_scope = scope.clone();
            target_scope.session_file = None;
            SessionOperationError {
                kind: SessionErrorKind::Identity,
                message: "Pi native fork returned a target identity without its session file"
                    .into(),
                retained: Some(Box::new(PreparedSession {
                    host_id: request.expected_host,
                    agent: CLIAgent::Pi,
                    source_id: request.source_id.clone(),
                    target_id: Some(id.clone()),
                    session_file: None,
                    cwd: request.cwd.clone(),
                    scope: target_scope,
                    launch_argv: launch.to_vec(),
                    command: None,
                    created: true,
                })),
            }
        })?;
    let file = resolve(cwd, Path::new(path));
    let identity_error = match header(&file) {
        Ok(persisted) if persisted == id => None,
        Ok(_) => Some("Pi fork reported an identity that differs from its persisted header".into()),
        Err(error) => Some(format!(
            "Pi fork reported identity but its persisted header could not be verified: {error}"
        )),
    };
    let error = if identity_error.is_some() {
        identity_error
    } else if timed_out {
        Some("Pi fork was created but its preparation process did not shut down in time".into())
    } else if !output.status.success() {
        Some(format!(
            "Pi fork was created but preparation exited unsuccessfully ({})",
            output.status
        ))
    } else {
        None
    };
    Ok((file, id, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(root: &Path) -> SessionOperation {
        let file = root.join("source.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"session\",\"version\":3,\"id\":\"real-source\",\"cwd\":\"/project\"}\n",
        )
        .unwrap();
        SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Pi,
            kind: SessionOperationKind::Resume,
            source_id: "real-source".into(),
            cwd: root.into(),
            launch_argv: vec![
                "pi".into(),
                "--continue".into(),
                "--resume".into(),
                "--session-id".into(),
                "stale".into(),
                "--fork".into(),
                "old-source".into(),
                "--model".into(),
                "provider/model".into(),
                "--extension".into(),
                "custom extension.ts".into(),
                "--session-dir".into(),
                root.to_string_lossy().into_owned(),
                "--".into(),
                "old initial task".into(),
            ],
            scope: SessionScope {
                session_file: Some(file),
                shell_program: Some("sh".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn pi_resume_binds_the_exact_source_and_preserves_its_store_and_custom_options() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::host::local::LocalHost::new();
        let mut operation = request(root.path());
        let effective = root
            .path()
            .join("effective-config")
            .to_string_lossy()
            .into_owned();
        operation.launch_argv.splice(
            0..0,
            [
                format!(
                    "PI_CODING_AGENT_DIR={}",
                    root.path().join("old-config").display()
                ),
                format!("PI_CODING_AGENT_DIR={effective}"),
            ],
        );
        let prepared = host.session_operation(&operation).unwrap();
        assert_eq!(prepared.target_id.as_deref(), Some("real-source"));
        assert!(!prepared.created);
        assert_eq!(prepared.scope.session_dir.as_deref(), Some(root.path()));
        assert_eq!(
            prepared.scope.environment.get("PI_CODING_AGENT_DIR"),
            Some(&effective)
        );
        let argv = crate::core::cli_agent::command_argv(prepared.command.as_deref().unwrap());
        for stale in [
            "--continue",
            "--resume",
            "--fork",
            "--session-id",
            "old initial task",
            "stale",
        ] {
            assert!(!argv.iter().any(|arg| arg == stale));
        }
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["--model", "provider/model"])
        );
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["--extension", "custom extension.ts"])
        );
        assert_eq!(
            prepared
                .opening_failed("pane creation failed")
                .retained
                .unwrap()
                .target_id
                .as_deref(),
            Some("real-source")
        );
    }

    #[test]
    fn pi_resume_preserves_explicit_image_protocol_without_omp_options() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::host::local::LocalHost::new();
        let mut operation = request(root.path());
        operation
            .launch_argv
            .insert(0, "PI_IMAGE_PROTOCOL=kitty".into());
        let prepared = host.session_operation(&operation).unwrap();
        assert_eq!(
            prepared
                .scope
                .environment
                .get("PI_IMAGE_PROTOCOL")
                .map(String::as_str),
            Some("kitty")
        );
        // A bound native scope wins over launch defaults, including disabling.
        operation
            .scope
            .environment
            .insert("PI_IMAGE_PROTOCOL".into(), "none".into());
        let disabled = host.session_operation(&operation).unwrap();
        assert_eq!(
            durable_scope(&disabled.scope)
                .environment
                .get("PI_IMAGE_PROTOCOL")
                .map(String::as_str),
            Some("none")
        );
        assert!(
            disabled
                .command
                .as_ref()
                .unwrap()
                .contains("PI_IMAGE_PROTOCOL")
        );
        operation.launch_argv[0] = "PI_FORCE_IMAGE_PROTOCOL=kitty".into();
        assert_eq!(
            host.session_operation(&operation).unwrap_err().kind,
            SessionErrorKind::Unsupported
        );
    }

    #[test]
    fn pi_missing_or_mismatched_source_never_prepares_an_empty_session() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::host::local::LocalHost::new();
        let mut operation = request(root.path());
        operation.source_id = "another-source".into();
        assert_eq!(
            host.session_operation(&operation).unwrap_err().kind,
            SessionErrorKind::Identity
        );
        operation.source_id = "real-source".into();
        operation.scope.session_file = Some(root.path().join("missing.jsonl"));
        assert_eq!(
            host.session_operation(&operation).unwrap_err().kind,
            SessionErrorKind::Identity
        );
        assert!(!root.path().join("missing.jsonl").exists());
    }

    #[test]
    #[ignore = "requires an isolated official Pi 1.3.0 install in LYREL_PI_NATIVE_ARGV (JSON argv)"]
    fn pi_native_fork_returns_independent_persisted_identity_before_tui_open() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::host::local::LocalHost::new();
        let mut operation = request(root.path());
        let source = operation.scope.session_file.clone().unwrap();
        operation.launch_argv = serde_json::from_str(
            &std::env::var("LYREL_PI_NATIVE_ARGV").expect("isolated Pi argv required"),
        )
        .unwrap();
        operation.launch_argv.push("--offline".into());
        operation.scope.session_dir = Some(root.path().join("sessions"));
        std::fs::create_dir_all(operation.scope.session_dir.as_ref().unwrap()).unwrap();
        // Keep the source in the same selected native storage scope.
        let stored_source = operation
            .scope
            .session_dir
            .as_ref()
            .unwrap()
            .join("source.jsonl");
        std::fs::rename(source, &stored_source).unwrap();
        operation.scope.session_file = Some(stored_source.clone());
        for key in ["HOME", "USERPROFILE", "PI_CODING_AGENT_DIR"] {
            operation
                .scope
                .environment
                .insert(key.into(), root.path().to_string_lossy().into_owned());
        }
        operation.kind = SessionOperationKind::Fork;
        let fork = host.session_operation(&operation).unwrap();
        assert!(fork.created);
        assert_ne!(fork.target_id.as_deref(), Some("real-source"));
        assert_eq!(
            header(fork.session_file.as_ref().unwrap()).unwrap(),
            fork.target_id.clone().unwrap()
        );
        assert_eq!(header(&stored_source).unwrap(), "real-source");
        let retained = fork
            .opening_failed("new pane could not open")
            .retained
            .unwrap();
        operation.kind = SessionOperationKind::Resume;
        operation.source_id = retained.target_id.clone().unwrap();
        operation.scope = retained.scope.clone();
        let reopen = host.session_operation(&operation).unwrap();
        assert_eq!(reopen.target_id, retained.target_id);
        assert!(!reopen.created);
    }

    #[test]
    fn pi_does_not_accept_omp_profiles_or_opencode_connections() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::host::local::LocalHost::new();
        let mut request = SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Pi,
            kind: SessionOperationKind::Resume,
            source_id: "source".into(),
            cwd: root.path().into(),
            launch_argv: vec!["pi".into()],
            scope: SessionScope::default(),
        };
        request.scope.profile = Some("omp-profile".into());
        assert_eq!(
            host.session_operation(&request).unwrap_err().kind,
            SessionErrorKind::Unsupported
        );
        request.scope.profile = None;
        request.scope.opencode = Some(Default::default());
        assert_eq!(
            host.session_operation(&request).unwrap_err().kind,
            SessionErrorKind::Unsupported
        );
    }
}
