//! Hermes 0.22.2 native stored-session operations via its official source-install launcher.
use super::*;

const HOME_ENV: &[&str] = &["HOME", "USERPROFILE", "HERMES_HOME", "HERMES_RUNTIME_DIR"];

pub(crate) fn sessions(
    request: &SessionOperation,
) -> Result<Vec<crate::core::agent_history::PastSession>, SessionOperationError> {
    if request.agent != CLIAgent::Hermes {
        return Err(failure(
            SessionErrorKind::Identity,
            "Hermes history requires a Hermes launch context",
        ));
    }
    // Executes only on the data-owning Host; the same context resolver pins
    // history, native preparation and the new CLI to one effective home.
    let (_, gateway, scope) = context(request)?;
    let (result, shutdown_error) = rpc(
        &gateway,
        &scope,
        &request.cwd,
        "projects.tree",
        serde_json::json!({"preview_limit":501,"session_limit":501}),
    )?;
    if let Some(error) = shutdown_error {
        return Err(failure(SessionErrorKind::Command, error));
    }
    let projects = result["projects"].as_array().ok_or_else(|| {
        failure(
            SessionErrorKind::Command,
            "Hermes history returned no project metadata",
        )
    })?;
    let mut sessions = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut metadata_count = 0;
    for project in projects {
        let rows = project["previewSessions"].as_array().ok_or_else(|| {
            failure(
                SessionErrorKind::Command,
                "Hermes history project has no session metadata",
            )
        })?;
        metadata_count += rows.len();
        if metadata_count >= 501 {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "Hermes history reached the 500-session metadata window; the official projects.tree endpoint has no offset/cursor. No complete history list was returned; use native Hermes history to inspect older sessions.",
            ));
        }
        for row in rows {
            if row["source"].as_str() != Some("cli") || row["archived"].as_bool() == Some(true) {
                continue;
            }
            let id = row["id"]
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(|| {
                    failure(
                        SessionErrorKind::Identity,
                        "Hermes history returned an invalid stored session identity",
                    )
                })?;
            if !ids.insert(id.to_owned()) {
                continue;
            }
            let title = row["title"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(id);
            let updated = row["last_active"]
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| {
                    failure(
                        SessionErrorKind::Command,
                        "Hermes history returned no activity timestamp",
                    )
                })?;
            sessions.push(crate::core::agent_history::PastSession {
                agent: CLIAgent::Hermes,
                id: id.into(),
                title: title.chars().take(120).collect(),
                cwd: row["cwd"]
                    .as_str()
                    .filter(|cwd| !cwd.is_empty())
                    .map(PathBuf::from),
                branch: row["git_branch"].as_str().map(str::to_owned),
                updated: updated as u64,
                scope: Some(scope.clone()),
            });
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated));
    sessions.truncate(500);
    Ok(sessions)
}

pub(super) fn prepare(
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    let (launch, gateway, scope) = context(request)?;
    // Validate shell representation before any irreversible creation.
    command_line(&launch, &scope)?;
    let (method, params) = match request.kind {
        SessionOperationKind::Fork => (
            "session.branch_stored",
            serde_json::json!({
                "parent_session_id": request.source_id, "source": "cli", "cwd": request.cwd,
            }),
        ),
        SessionOperationKind::Resume => (
            "session.resume",
            serde_json::json!({
                "session_id": request.source_id, "source": "cli", "lazy": true,
                "defer_history": true, "omit_messages": true, "eager_build": false,
            }),
        ),
    };
    let (result, shutdown_error) = rpc(&gateway, &scope, &request.cwd, method, params)?;
    // The short runtime session_id cannot be used to resume the persisted CLI session.
    let identity_key = match request.kind {
        SessionOperationKind::Fork => "stored_session_id",
        SessionOperationKind::Resume => "session_key",
    };
    let target = result[identity_key].as_str().filter(|id| valid_id(id))
        .ok_or_else(|| failure(SessionErrorKind::Identity,
            "Hermes returned no persisted session identity; native creation may have succeeded — inspect native history before repeating the operation"))?;
    if request.kind == SessionOperationKind::Fork && target == request.source_id {
        return Err(failure(
            SessionErrorKind::Identity,
            "Hermes did not return an independent stored fork identity",
        ));
    }
    if request.kind == SessionOperationKind::Resume && target != request.source_id {
        return Err(failure(
            SessionErrorKind::Identity,
            "Hermes resolved a different stored session than the selected source",
        ));
    }
    let mut open = launch.clone();
    open.extend([
        "--resume".into(),
        target.into(),
        "--in".into(),
        request.cwd.to_string_lossy().into_owned(),
    ]);
    let command = command_line(&open, &scope);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: CLIAgent::Hermes,
        source_id: request.source_id.clone(),
        target_id: Some(target.into()),
        session_file: None,
        cwd: request.cwd.clone(),
        scope,
        launch_argv: launch,
        command: command.as_ref().ok().cloned(),
        created: request.kind == SessionOperationKind::Fork,
    };
    if let Err(error) = command {
        let kind = error.kind;
        let mut retained = prepared.opening_failed(error.message);
        retained.kind = kind;
        return Err(retained);
    }
    if let Some(message) = shutdown_error {
        let mut error = prepared.opening_failed(message);
        error.kind = SessionErrorKind::Command;
        return Err(error);
    }
    Ok(prepared)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn context(
    request: &SessionOperation,
) -> Result<(Vec<String>, Vec<String>, SessionScope), SessionOperationError> {
    let mut scope = request.scope.clone();
    if scope
        .environment
        .keys()
        .any(|key| !HOME_ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Hermes accepts only home/runtime scope environment selectors",
        ));
    }
    let mut argv = request.launch_argv.as_slice();
    while let Some((key, value)) = argv.first().and_then(|arg| arg.split_once('=')) {
        if !HOME_ENV.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "unsupported Hermes launch environment selector",
            ));
        }
        scope
            .environment
            .entry(key.into())
            .or_insert_with(|| value.into());
        argv = &argv[1..];
    }
    if argv.is_empty() {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Hermes launch executable was not captured",
        ));
    }
    let named = (0..argv.len())
        .find(|i| CLIAgent::detect_from_argv(&argv[..=*i]) == Some(CLIAgent::Hermes))
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Unsupported,
                "cannot identify the Hermes executable in the captured launch context",
            )
        })?;
    let mut launch = argv[..=named].to_vec();
    let mut gateway = launch.clone();
    launch.push("chat".into());
    let values = [
        "--model",
        "-m",
        "--provider",
        "--reasoning",
        "--toolsets",
        "-t",
        "--skills",
        "-s",
        "--max-turns",
        "--run-budget",
        "--source",
    ];
    let switches = [
        "--verbose",
        "-v",
        "--checkpoints",
        "--yolo",
        "--pass-session-id",
        "--ignore-rules",
        "--cli",
        "--tui",
        "--native",
        "--tui-native",
        "--dev",
        "--accept-hooks",
    ];
    let mut i = named + 1;
    while i < argv.len() {
        let arg = &argv[i];
        let key = arg.split('=').next().unwrap_or(arg);
        if arg == "chat" {
            i += 1;
            continue;
        }
        if matches!(key, "--safe-mode" | "--ignore-user-config") {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "Hermes customization-disabled mode cannot load the authorized lyrel plugin",
            ));
        }
        if matches!(key, "--profile" | "-p") {
            let value = option_value(argv, &mut i)?;
            if request.scope.profile.is_none() {
                scope.profile = Some(value);
            }
        } else if matches!(
            key,
            "--resume"
                | "-r"
                | "--in"
                | "--query"
                | "-q"
                | "--query-file"
                | "--image"
                | "--usage-file"
                | "--format"
        ) {
            option_value(argv, &mut i)?;
        } else if matches!(key, "--continue" | "-c") {
            if !arg.contains('=') && argv.get(i + 1).is_some_and(|value| !value.starts_with('-')) {
                i += 1;
            }
        } else if matches!(
            key,
            "--oneshot"
                | "-z"
                | "--quiet"
                | "-Q"
                | "--worktree"
                | "-w"
                | "--no-restore-cwd"
                | "--create-if-missing"
        ) {
            // Never replay prompts or actions that create/switch a second workspace.
            if key == "-z" {
                option_value(argv, &mut i)?;
            }
        } else if values.contains(&key) {
            let value = option_value(argv, &mut i)?;
            launch.extend([key.into(), value]);
        } else if switches.contains(&key) {
            launch.push(arg.clone());
        } else {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "Hermes launch contains an unsupported interactive replay option",
            ));
        }
        i += 1;
    }
    if !scope.environment.contains_key("HERMES_HOME") {
        let host = crate::host::local::LocalHost::new();
        let home = crate::core::agent_history::home()
            .ok_or_else(|| failure(SessionErrorKind::Directory, "Hermes Host has no home"))?;
        let mut args = scope
            .environment
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>();
        args.extend(launch.iter().cloned());
        if let Some(profile) = &scope.profile {
            args.extend(["--profile".into(), profile.clone()]);
        }
        let target = crate::core::agent_hooks::HookTarget::remote(&*host, home)
            .with_launch_arguments(crate::core::agent_hooks::HookAgent::Hermes, &args);
        let home = target.hermes_home().map_err(|e| {
            failure(
                SessionErrorKind::Directory,
                format!("Hermes scope unavailable: {e}"),
            )
        })?;
        scope
            .environment
            .insert("HERMES_HOME".into(), home.to_string_lossy().into_owned());
    }
    for key in HOME_ENV {
        if !scope.environment.contains_key(*key)
            && let Ok(value) = std::env::var(key)
        {
            scope.environment.insert((*key).into(), value);
        }
    }
    // The gateway bypasses normal CLI sticky-profile processing. The observer's
    // fully resolved HERMES_HOME must agree with the reported profile, and the
    // new CLI needs an explicit selector so a later `profile use` cannot move it.
    let profile = if let Some(home) = scope
        .environment
        .get("HERMES_HOME")
        .filter(|value| !value.is_empty())
    {
        let mut parts = home.trim_end_matches(['/', '\\']).rsplit(['/', '\\']);
        let name = parts.next().unwrap_or("");
        let actual = if parts.next() == Some("profiles") {
            name
        } else {
            "default"
        };
        if scope
            .profile
            .as_deref()
            .is_some_and(|profile| profile != actual)
        {
            return Err(failure(
                SessionErrorKind::Identity,
                "Hermes profile does not match the observed resolved HERMES_HOME",
            ));
        }
        actual.to_owned()
    } else {
        if scope
            .profile
            .as_deref()
            .is_some_and(|profile| profile != "default")
        {
            return Err(failure(
                SessionErrorKind::Identity,
                "Hermes named profile requires the observer's resolved HERMES_HOME before session preparation",
            ));
        }
        "default".into()
    };
    scope.profile = Some(profile.clone());
    launch.extend(["--profile".into(), profile]);
    gateway.extend([
        "--print-runtime-command".into(),
        "--module".into(),
        "tui_gateway.entry".into(),
    ]);
    Ok((launch, gateway, scope))
}

fn option_value(argv: &[String], index: &mut usize) -> Result<String, SessionOperationError> {
    if let Some((_, value)) = argv[*index].split_once('=') {
        return Ok(value.into());
    }
    *index += 1;
    argv.get(*index).cloned().ok_or_else(|| {
        failure(
            SessionErrorKind::Unsupported,
            "incomplete Hermes launch option",
        )
    })
}

fn runtime_command(
    launcher: &[String],
    scope: &SessionScope,
    cwd: &Path,
) -> Result<Vec<String>, SessionOperationError> {
    let mut command = Command::new(&launcher[0]);
    command
        .args(&launcher[1..])
        .current_dir(cwd)
        .envs(&scope.environment)
        .env_remove("LYREL")
        .env_remove(crate::daemon::pane::LYREL_PANE_ENV);
    crate::core::proc::hide_console(&mut command);
    let output =
        crate::core::proc::output_within(&mut command, Duration::from_secs(30)).map_err(|e| {
            failure(
                SessionErrorKind::Command,
                format!("Hermes runtime command resolution failed: {e}"),
            )
        })?;
    if !output.status.success() {
        return Err(failure(
            SessionErrorKind::Unsupported,
            format!(
                "The selected Hermes launcher does not provide --print-runtime-command --module tui_gateway.entry ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(1000)
                    .collect::<String>()
            ),
        ));
    }
    let argv: Vec<String> = serde_json::from_slice(&output.stdout).map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "Hermes launcher returned no valid runtime argv JSON",
        )
    })?;
    if argv.first().is_none_or(|value| value.is_empty()) {
        return Err(failure(
            SessionErrorKind::Command,
            "Hermes launcher returned an empty runtime command",
        ));
    }
    Ok(argv)
}

fn rpc(
    gateway: &[String],
    scope: &SessionScope,
    cwd: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<(serde_json::Value, Option<String>), SessionOperationError> {
    // Resolve the official Python/bootstrap argv so Windows .cmd wrappers do
    // not retain ownership of a long-lived gateway child on timeout.
    let runtime = runtime_command(gateway, scope, cwd)?;
    let mut command = Command::new(&runtime[0]);
    command
        .args(&runtime[1..])
        .current_dir(cwd)
        .envs(&scope.environment)
        .env_remove("LYREL")
        .env_remove(crate::daemon::pane::LYREL_PANE_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::core::proc::hide_console(&mut command);
    let mut child = command.spawn().map_err(|e| {
        failure(
            SessionErrorKind::Command,
            format!("Hermes resolved gateway could not start: {e}"),
        )
    })?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (error_tx, error_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        let mut frame = Vec::new();
        loop {
            frame.clear();
            if !matches!(reader.by_ref().take(1024 * 1024 + 1).read_until(b'\n', &mut frame), Ok(n) if n > 0)
                || frame.len() > 1024 * 1024
            {
                break;
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&frame)
                && matches!(
                    value["id"].as_str(),
                    Some("lyrel-hermes-operation" | "lyrel-hermes-close")
                )
                && value["jsonrpc"] == "2.0"
            {
                let _ = tx.send(value);
            }
        }
        let _ = io::copy(&mut reader, &mut io::sink());
    });
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(64 * 1024).read_to_end(&mut bytes);
        let _ = error_tx.send(bytes);
        let _ = io::copy(&mut stderr, &mut io::sink());
    });
    let frame = serde_json::json!({"jsonrpc":"2.0", "id":"lyrel-hermes-operation", "method":method, "params":params});
    let response_deadline = Instant::now() + Duration::from_secs(45);
    let mut stdin = child.stdin.take().unwrap();
    if let Err(error) = writeln!(stdin, "{frame}").and_then(|_| stdin.flush()) {
        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure(
            SessionErrorKind::Command,
            format!("Hermes gateway request failed: {error}"),
        ));
    }
    let response = rx.recv_timeout(response_deadline.saturating_duration_since(Instant::now()));
    // The persisted target belongs to the caller, but the short-lived runtime
    // session belongs to this gateway client. Release it through the public
    // lifecycle before EOF instead of leaving teardown to interpreter shutdown.
    let close_error = response
        .as_ref()
        .ok()
        .and_then(|response| response["result"]["session_id"].as_str())
        .filter(|session_id| !session_id.is_empty())
        .and_then(|session_id| {
            let close = serde_json::json!({
                "jsonrpc": "2.0", "id": "lyrel-hermes-close", "method": "session.close",
                "params": {"session_id": session_id},
            });
            if let Err(error) = writeln!(stdin, "{close}").and_then(|_| stdin.flush()) {
                return Some(format!("Hermes returned a native result but session.close failed: {error}"));
            }
            match rx.recv_timeout(response_deadline.saturating_duration_since(Instant::now())) {
                Ok(response) if response["id"] == "lyrel-hermes-close"
                    && response["result"]["closed"] == true => None,
                Ok(response) => Some(format!(
                    "Hermes returned a native result but session.close did not release the runtime session: {}",
                    response["error"]["message"].as_str().unwrap_or("native close was not confirmed")
                )),
                Err(error) => Some(format!(
                    "Hermes returned a native result but session.close returned no response: {error}"
                )),
            }
        });
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let response = response.map_err(|_| {
        let stderr = error_rx
            .recv_timeout(Duration::from_millis(100))
            .unwrap_or_default();
        let ambiguity = if method == "session.branch_stored" {
            " Native creation may already have succeeded; inspect history before retrying."
        } else {
            ""
        };
        failure(
            SessionErrorKind::Command,
            format!(
                "Hermes {method} returned no response in this home/profile.{ambiguity} {}",
                String::from_utf8_lossy(&stderr)
                    .chars()
                    .take(1000)
                    .collect::<String>()
            ),
        )
    })?;
    if let Some(error) = response.get("error") {
        return Err(failure(
            SessionErrorKind::Command,
            format!(
                "Hermes {method}: {}",
                error["message"].as_str().unwrap_or("native protocol error")
            ),
        ));
    }
    let result = response.get("result").cloned().ok_or_else(|| {
        failure(
            SessionErrorKind::Command,
            "Hermes gateway returned neither result nor error",
        )
    })?;
    let shutdown_error = match status {
        None => Some("Hermes returned a native result but gateway shutdown timed out".into()),
        Some(status) if !status.success() => Some(format!(
            "Hermes returned a native result but gateway exited unsuccessfully ({status})"
        )),
        _ => None,
    };
    Ok((result, close_error.or(shutdown_error)))
}
