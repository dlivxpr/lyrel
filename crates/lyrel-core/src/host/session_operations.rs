//! Host-owned session operations. Preparation is blocking and pane-independent;
//! consumers retain a PreparedSession until its new pane has opened successfully.

mod pi;

use std::collections::BTreeMap;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::core::cli_agent::CLIAgent;
use crate::core::shell_quote::{Quoting, quote_for_shell, quoting_for};
use crate::host::{Host, guard_off_ui};

pub(crate) mod hermes;
#[cfg(test)]
mod hermes_tests;
mod kimi;
pub mod opencode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionOperationKind {
    Resume,
    Fork,
}

/// Only scope selectors, never credentials. An absent selector inherits the
/// executing Host's environment, not the GUI machine's environment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionScope {
    pub profile: Option<String>,
    pub session_dir: Option<PathBuf>,
    pub session_file: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub shell_program: Option<String>,
    pub opencode: Option<opencode::OpenCodeScope>,
}

/// The one persistence rule for every adapter: retain native scope selectors,
/// never a client lease or credential. Runtime readouts keep their live bridge.
pub fn durable_scope(scope: &SessionScope) -> SessionScope {
    let mut scope = scope.clone();
    scope
        .environment
        .retain(|key, _| SCOPE_ENV.contains(&key.as_str()));
    if let Some(opencode) = &mut scope.opencode {
        opencode.bridge = None;
        // pid/advertised URLs identify a live process, not durable storage.
        // Cold resume rebinds through the saved Host-native connection selector.
        opencode.server = None;
        opencode.launch_argv.clear();
    }
    scope
}

pub fn serialize_durable_scope<S: serde::Serializer>(
    scope: &Option<SessionScope>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    scope.as_ref().map(durable_scope).serialize(serializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOperation {
    pub expected_host: u64,
    pub agent: CLIAgent,
    pub kind: SessionOperationKind,
    pub source_id: String,
    pub cwd: PathBuf,
    pub launch_argv: Vec<String>,
    pub scope: SessionScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedSession {
    pub host_id: u64,
    pub agent: CLIAgent,
    pub source_id: String,
    /// None only for an adapter whose native TUI creates the fork later.
    /// This is not a fabricated identity and is not a completed fork.
    pub target_id: Option<String>,
    pub session_file: Option<PathBuf>,
    pub cwd: PathBuf,
    pub scope: SessionScope,
    pub launch_argv: Vec<String>,
    /// Absent only in an error's retained result when launch preparation failed.
    pub command: Option<String>,
    pub created: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionErrorKind {
    WrongHost,
    Directory,
    Identity,
    Unsupported,
    Command,
    Connection,
    Launch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOperationError {
    pub kind: SessionErrorKind,
    pub message: String,
    /// A successfully created fork is never deleted or created again on retry.
    pub retained: Option<Box<PreparedSession>>,
}

impl std::fmt::Display for SessionOperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(prepared) = &self.retained
            && let Some(target) = &prepared.target_id
        {
            let object = if prepared.created {
                "fork retained"
            } else {
                "session retained"
            };
            write!(f, " ({object}: {target})")?;
        }
        Ok(())
    }
}
impl std::error::Error for SessionOperationError {}

impl PreparedSession {
    pub fn opening_failed(self, message: impl Into<String>) -> SessionOperationError {
        SessionOperationError {
            kind: SessionErrorKind::Launch,
            message: message.into(),
            retained: Some(Box::new(self)),
        }
    }
}

/// Encode the reported native context in the existing persisted argv field.
/// Only documented native flags and non-credential environment selectors enter it.
pub fn launch_context(
    agent: CLIAgent,
    state: &crate::core::cli_agent::AgentSessionState,
) -> Option<Vec<String>> {
    let mut argv = state.launch_argv.clone()?;

    if agent == CLIAgent::OpenCode {
        let presence = state.readout.opencode_presence.as_ref()?;
        if presence.error.is_some() {
            return None;
        }
        // The captured process argv may predate a client reconnect. Only this
        // TUI's authoritative non-secret connection selection is replayed.
        let mut out = Vec::with_capacity(argv.len() + presence.connection_argv.len());
        let mut i = 0;
        while i < argv.len() {
            let arg = &argv[i];
            if arg == "--server" {
                i += 2;
                continue;
            }
            if arg.starts_with("--server=") || arg == "--standalone" {
                i += 1;
                continue;
            }
            out.push(arg.clone());
            i += 1;
        }
        out.extend(presence.connection_argv.iter().cloned());
        return Some(out);
    }
    if !matches!(
        agent,
        CLIAgent::OhMyPi | CLIAgent::Pi | CLIAgent::Kimi | CLIAgent::Hermes
    ) {
        return Some(argv);
    }
    let Some(scope) = &state.readout.session_scope else {
        return Some(argv);
    };
    let prefix = argv
        .iter()
        .take_while(|arg| {
            arg.split_once('=')
                .is_some_and(|(key, _)| SCOPE_ENV.contains(&key))
        })
        .count();
    let mut out = argv
        .drain(..prefix)
        .filter(|arg| {
            !arg.split_once('=')
                .is_some_and(|(key, _)| scope.environment.contains_key(key))
        })
        .collect::<Vec<_>>();
    out.extend(
        scope
            .environment
            .iter()
            .filter(|(key, _)| SCOPE_ENV.contains(&key.as_str()))
            .map(|(key, value)| format!("{key}={value}")),
    );
    out.extend(argv);
    if matches!(agent, CLIAgent::Hermes | CLIAgent::Kimi) {
        return Some(out);
    }
    if let Some(profile) = &scope.profile {
        out.extend(["--profile".into(), profile.clone()]);
    }
    if let Some(dir) = &scope.session_dir {
        out.extend(["--session-dir".into(), dir.to_string_lossy().into_owned()]);
    }
    if let Some(file) = &scope.session_file {
        out.extend([
            if agent == CLIAgent::Pi {
                "--session"
            } else {
                "--resume"
            }
            .into(),
            file.to_string_lossy().into_owned(),
        ]);
    }
    Some(out)
}

fn failure(kind: SessionErrorKind, message: impl Into<String>) -> SessionOperationError {
    SessionOperationError {
        kind,
        message: message.into(),
        retained: None,
    }
}

pub(crate) fn prepare<H: Host + ?Sized>(
    host: &H,
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    guard_off_ui();
    if host.id().0 != request.expected_host {
        return Err(failure(
            SessionErrorKind::WrongHost,
            "session belongs to another host",
        ));
    }
    if !host.is_connected() {
        return Err(failure(
            SessionErrorKind::Connection,
            "session host is disconnected",
        ));
    }
    if !host.is_absolute(&request.cwd)
        || !host
            .stat(&request.cwd)
            .map_err(|e| {
                failure(
                    SessionErrorKind::Directory,
                    format!("{}: {e}", request.cwd.display()),
                )
            })?
            .is_dir
    {
        return Err(failure(
            SessionErrorKind::Directory,
            "session working directory is not an absolute directory",
        ));
    }
    if request.source_id.is_empty()
        || request.source_id.starts_with('-')
        || !request
            .source_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(failure(
            SessionErrorKind::Identity,
            "invalid session identity",
        ));
    }
    if request.agent != CLIAgent::Pi && request.launch_argv.iter().any(|arg| arg == "--no-session")
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "session persistence is disabled",
        ));
    }
    // Resolve an unspecified shell on the executing Host, never on the client.
    let mut resolved;
    let request = if request.scope.shell_program.is_none() {
        resolved = request.clone();
        resolved.scope.shell_program = Some(default_shell_program(host)?);
        &resolved
    } else {
        request
    };
    match request.agent {
        CLIAgent::OhMyPi => omp(request),
        CLIAgent::Pi => pi::prepare(host, request),
        CLIAgent::Kimi => kimi::prepare(host, request),
        CLIAgent::OpenCode => opencode::prepare(request),
        CLIAgent::Hermes => hermes::prepare(request),
    }
}

// One source for replay, Host discovery and the generated Pi/OMP bridges.
// The macro only concatenates static slices; no runtime registry or allocation.
macro_rules! scope_environment_keys {
    (common: [$($common:literal),* $(,)?], pi: [$($pi:literal),* $(,)?], omp: [$($omp:literal),* $(,)?], other: [$($other:literal),* $(,)?]) => {
        pub(crate) const PI_SCOPE_ENV: &[&str] = &[$($common,)* $($pi,)*];
        pub(crate) const OMP_SCOPE_ENV: &[&str] = &[$($common,)* $($omp,)*];
        pub(crate) const SCOPE_ENV: &[&str] = &[$($common,)* $($pi,)* $($omp,)* $($other,)*];
    };
}

scope_environment_keys! {
    common: ["HOME", "USERPROFILE", "PI_CODING_AGENT_DIR"],
    pi: ["PI_CODING_AGENT_SESSION_DIR", "PI_IMAGE_PROTOCOL"],
    omp: [
        "OMP_PROFILE", "PI_PROFILE", "PI_CONFIG_DIR", "PI_CONFIG_FILES",
        "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME",
        "PI_FORCE_IMAGE_PROTOCOL", "PI_KITTY_PLACEHOLDERS", "PI_NO_KITTY_PLACEHOLDERS",
    ],
    other: ["KIMI_CODE_HOME", "OPENCODE_DB", "HERMES_HOME", "HERMES_RUNTIME_DIR"]
}

fn omp(request: &SessionOperation) -> Result<PreparedSession, SessionOperationError> {
    let mut scope = request.scope.clone();
    if scope
        .environment
        .keys()
        .any(|key| !SCOPE_ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "session scope accepts only documented home/profile/storage environment selectors",
        ));
    }
    let mut argv = request.launch_argv.clone();
    let prefix = scope_environment_prefix(&argv, &mut scope.environment)?;
    argv.drain(..prefix);
    if argv.is_empty() {
        argv.push("omp".into());
    }
    if scope.session_file.is_none() {
        for (index, arg) in argv.iter().enumerate() {
            let value = if matches!(arg.as_str(), "--resume" | "--session" | "-r") {
                argv.get(index + 1).map(String::as_str)
            } else {
                arg.strip_prefix("--resume=")
                    .or_else(|| arg.strip_prefix("--session="))
            };
            if let Some(value) =
                value.filter(|value| value.ends_with(".jsonl") || value.contains(['/', '\\']))
            {
                scope.session_file = Some(resolve(&request.cwd, Path::new(value)));
            }
        }
    }
    let named = (0..argv.len())
        .find(|i| CLIAgent::detect_from_argv(&argv[..=*i]) == Some(CLIAgent::OhMyPi))
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Unsupported,
                "cannot identify the omp executable in the captured launch context",
            )
        })?;
    let tail = CLIAgent::OhMyPi.session_free_tail(&argv).ok_or_else(|| {
        failure(
            SessionErrorKind::Unsupported,
            "cannot recover omp launch arguments",
        )
    })?;
    let mut launch = argv[..=named].to_vec();
    // Native argument values remain argv, including spaces and Unicode. Only
    // startup actions/prompts and transport selectors are removed.
    let values = [
        "--cwd",
        "--profile",
        "--config",
        "--session-dir",
        "--add-dir",
        "--model",
        "--smol",
        "--slow",
        "--plan",
        "--models",
        "--provider",
        "--thinking",
        "--tools",
        "--approval-mode",
        "--service-tier",
        "--provider-session-id",
        "--prompt-cache-key",
        "--extension",
        "-e",
        "--hook",
        "--trusted-extension",
        "--plugin-dir",
        "--skills",
        "--system-prompt",
        "--system-prompt-template",
        "--append-system-prompt",
        "--max-time",
    ];
    let switches = [
        "--allow-home",
        "--no-title",
        "--hide-thinking",
        "--print-thoughts",
        "--external-thinking",
        "--no-prewalk",
        "--no-tools",
        "--no-lsp",
        "--no-pty",
        "--auto-approve",
        "--yolo",
        "--advisor",
        "--no-extensions",
        "--no-skills",
        "--no-rules",
    ];
    let mut i = 0;
    while i < tail.len() {
        let arg = tail[i];
        if arg == "--" {
            break;
        }
        let key = arg.split('=').next().unwrap_or(arg);
        if key == "--api-key" {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "credential arguments cannot be replayed; configure credentials on the target host",
            ));
        }
        if matches!(
            key,
            "--mode"
                | "--goal"
                | "--export"
                | "--alias"
                | "--cwd"
                | "--prewalk-into"
                | "--plan-yolo-into"
        ) {
            i += if arg.contains('=') { 1 } else { 2 };
            continue;
        }
        if matches!(
            key,
            "--print"
                | "-p"
                | "--from-claude"
                | "--from-codex"
                | "--no-ui"
                | "--prewalk"
                | "--plan-yolo"
        ) || !arg.starts_with('-')
        {
            i += 1;
            continue;
        }
        if !values.contains(&key) && !switches.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "omp launch contains an option whose interactive replay semantics are not supported",
            ));
        }
        let value = if values.contains(&key) {
            Some(if let Some((_, value)) = arg.split_once('=') {
                value
            } else {
                i += 1;
                *tail.get(i).ok_or_else(|| {
                    failure(
                        SessionErrorKind::Unsupported,
                        "incomplete omp launch option",
                    )
                })?
            })
        } else {
            None
        };
        match (key, value) {
            ("--profile", Some(value)) => {
                if request.scope.profile.is_none() {
                    scope.profile = Some(value.into());
                }
            }
            ("--session-dir", Some(value)) => {
                if request.scope.session_dir.is_none() {
                    scope.session_dir = Some(resolve(&request.cwd, Path::new(value)));
                }
            }
            _ => {
                launch.push(arg.into());
                if !arg.contains('=')
                    && let Some(value) = value
                {
                    launch.push(value.into());
                }
            }
        }
        i += 1;
    }
    for key in SCOPE_ENV {
        if !scope.environment.contains_key(*key)
            && let Ok(value) = std::env::var(key)
        {
            scope.environment.insert((*key).into(), value);
        }
    }
    if let Some(profile) = &scope.profile {
        launch.extend(["--profile".into(), profile.clone()]);
    }
    if let Some(dir) = &scope.session_dir {
        launch.extend(["--session-dir".into(), dir.to_string_lossy().into_owned()]);
    }
    launch.extend(["--cwd".into(), request.cwd.to_string_lossy().into_owned()]);
    // Reject unrepresentable shell contexts before an irreversible native fork.
    command_line(&launch, &scope)?;
    // Source lookup must not start OMP: session_start extensions can append,
    // making a second writer move off an active source into a new sibling.
    let source_file = if let Some(file) = &scope.session_file {
        resolve(&request.cwd, file)
    } else {
        let files = crate::core::agent_history::omp_session_paths(&scope, &request.cwd)
            .map_err(|error| failure(SessionErrorKind::Directory, error.to_string()))?;
        let mut selected = None;
        for file in files {
            if omp_identity(&file).is_ok_and(|identity| identity == request.source_id) {
                if selected.is_some() {
                    return Err(failure(
                        SessionErrorKind::Identity,
                        "omp session ID is ambiguous in this storage scope; select its exact history entry",
                    ));
                }
                selected = Some(file);
            }
        }
        selected.ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "omp session does not exist in the selected storage scope",
            )
        })?
    };
    let source_id = omp_identity(&source_file)?;
    if source_id != request.source_id {
        return Err(failure(
            SessionErrorKind::Identity,
            "omp resumed a different session than the selected source",
        ));
    }
    if let Some(dir) = &scope.session_dir {
        let root = std::fs::canonicalize(dir)
            .map_err(|e| failure(SessionErrorKind::Directory, e.to_string()))?;
        let path = std::fs::canonicalize(&source_file)
            .map_err(|e| failure(SessionErrorKind::Identity, e.to_string()))?;
        if !path.starts_with(root) {
            return Err(failure(
                SessionErrorKind::Identity,
                "omp source is outside the selected session storage scope",
            ));
        }
    }
    // Pin future opens to the directory selected by the read-only lookup.
    // A later profile/default change must not resolve the same ID elsewhere.
    if scope.session_dir.is_none() {
        scope.session_dir = source_file.parent().map(Path::to_path_buf);
        if let Some(dir) = &scope.session_dir {
            launch.extend(["--session-dir".into(), dir.to_string_lossy().into_owned()]);
        }
    }
    let mut source_open = launch.clone();
    source_open.extend([
        "--resume".into(),
        source_file.to_string_lossy().into_owned(),
    ]);
    command_line(&source_open, &scope)?;
    let (session_file, target_id, shutdown_error) = if request.kind == SessionOperationKind::Fork {
        omp_state(&launch, &scope, &request.cwd, "--fork", &source_file.to_string_lossy())
            .map_err(|mut error| {
                error.message = format!("creating omp fork: {}; native creation may already have succeeded — inspect native history before repeating the fork", error.message);
                error
            })?
    } else {
        (source_file, source_id, None)
    };
    if request.kind == SessionOperationKind::Fork && target_id == request.source_id {
        return Err(failure(
            SessionErrorKind::Identity,
            "omp did not create an independent fork identity",
        ));
    }
    scope.session_file = Some(session_file.clone());
    let mut open = launch.clone();
    open.extend([
        "--resume".into(),
        session_file.to_string_lossy().into_owned(),
    ]);
    let command = command_line(&open, &scope);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: request.agent,
        source_id: request.source_id.clone(),
        target_id: Some(target_id),
        session_file: Some(session_file),
        cwd: request.cwd.clone(),
        scope,
        launch_argv: launch,
        command: command.as_ref().ok().cloned(),
        created: request.kind == SessionOperationKind::Fork,
    };
    if let Err(error) = command {
        let kind = error.kind;
        let mut error = prepared.opening_failed(error.to_string());
        error.kind = kind;
        return Err(error);
    }
    match shutdown_error {
        Some(message) => {
            let mut error = prepared.opening_failed(message);
            error.kind = SessionErrorKind::Command;
            Err(error)
        }
        None => Ok(prepared),
    }
}

fn resolve(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        cwd.join(path)
    }
}

fn default_shell_program<H: Host + ?Sized>(host: &H) -> Result<String, SessionOperationError> {
    let inventory = host.shells().map_err(|error| {
        failure(
            SessionErrorKind::Launch,
            format!("cannot resolve the launch host's default shell: {error}"),
        )
    })?;
    inventory
        .shells
        .into_iter()
        .find(|shell| shell.label == inventory.default_name && !shell.program.trim().is_empty())
        .map(|shell| shell.program)
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Launch,
                "the launch host did not identify its default shell",
            )
        })
}

/// Consume only literal public selectors. Explicit session scope wins over the
/// launch prefix; within the prefix, the last assignment wins like a shell.
fn scope_environment_prefix(
    argv: &[String],
    environment: &mut BTreeMap<String, String>,
) -> Result<usize, SessionOperationError> {
    let prefix = argv.iter().take_while(|arg| arg.contains('=')).count();
    for arg in &argv[..prefix] {
        let (key, _) = arg.split_once('=').unwrap();
        if !SCOPE_ENV.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "launch accepts only documented non-secret scope selectors",
            ));
        }
    }
    for arg in argv[..prefix].iter().rev() {
        let (key, value) = arg.split_once('=').unwrap();
        if !environment.contains_key(key) {
            environment.insert(key.into(), value.into());
        }
    }
    Ok(prefix)
}

/// Render literal public environment prefixes for the pane's actual shell.
/// Ordinary shell programs/wrappers are left intact; no image policy is added.
pub fn scoped_launch_command<H: Host + ?Sized>(
    host: &H,
    line: &str,
    shell: Option<&str>,
) -> Result<String, SessionOperationError> {
    let Ok(mut argv) = crate::core::cli_agent::parse_command_argv(line) else {
        return Ok(line.into());
    };
    if !argv.first().is_some_and(|arg| {
        arg.split_once('=')
            .is_some_and(|(key, _)| SCOPE_ENV.contains(&key))
    }) {
        return Ok(line.into());
    }
    let mut scope = SessionScope {
        shell_program: Some(match shell {
            Some(shell) => shell.to_owned(),
            None => default_shell_program(host)?,
        }),
        ..Default::default()
    };
    let prefix = scope_environment_prefix(&argv, &mut scope.environment)?;
    argv.drain(..prefix);
    if argv.is_empty() {
        return Err(failure(
            SessionErrorKind::Command,
            "launch environment has no executable",
        ));
    }
    command_line(&argv, &scope)
}

pub(crate) fn command_line(
    argv: &[String],
    scope: &SessionScope,
) -> Result<String, SessionOperationError> {
    let shell = scope.shell_program.as_deref();
    let quoting = quoting_for(shell);
    if quoting == Quoting::Cmd
        && argv
            .iter()
            .chain(scope.environment.values())
            .any(|arg| arg.contains(['"', '%', '!', '\r', '\n']))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "this launch context cannot be represented safely in cmd.exe",
        ));
    }
    let quote = |value: &str| quote_for_shell(value, shell);
    let mut command = argv
        .iter()
        .map(|value| quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    if quoting == Quoting::PowerShell {
        command = format!("& {command}");
    }
    if quoting == Quoting::PowerShell && !scope.environment.is_empty() {
        let mut save = String::new();
        let mut restore = String::new();
        for key in scope.environment.keys() {
            save.push_str(&format!(
                "$__lyrelEnv['{key}']=[Environment]::GetEnvironmentVariable('{key}','Process'); "
            ));
            restore.push_str(&format!(
                "[Environment]::SetEnvironmentVariable('{key}',$__lyrelEnv['{key}'],'Process'); "
            ));
        }
        for (key, value) in &scope.environment {
            save.push_str(&format!(
                "[Environment]::SetEnvironmentVariable('{key}','{}','Process'); ",
                value.replace('\'', "''")
            ));
        }
        // Scriptblock-local bookkeeping, with restoration even on launch failure.
        return Ok(format!(
            "& {{ $__lyrelEnv=@{{}}; {save}try {{ {command} }} finally {{ {restore} }} }}"
        ));
    }
    for (key, value) in &scope.environment {
        command = match quoting {
            Quoting::Cmd => format!("set \"{key}={value}\" && {command}"),
            Quoting::Posix => format!("{key}={} {command}", quote(value)),
            Quoting::PowerShell => unreachable!(),
        };
    }
    if quoting == Quoting::Cmd && !scope.environment.is_empty() {
        command = format!("cmd.exe /D /V:OFF /S /C \"{command}\"");
    }
    Ok(command)
}

fn omp_identity(path: &Path) -> Result<String, SessionOperationError> {
    let file = std::fs::File::open(path).map_err(|e| {
        failure(
            SessionErrorKind::Identity,
            format!("cannot read persisted omp identity: {e}"),
        )
    })?;
    let mut reader = io::BufReader::new(file).take(256 * 1024);
    let mut line = String::new();
    // Native OMP can reserve its first line for the in-place title slot.
    for _ in 0..2 {
        line.clear();
        reader.read_line(&mut line).map_err(|e| {
            failure(
                SessionErrorKind::Identity,
                format!("cannot read persisted omp identity: {e}"),
            )
        })?;
        let header: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
            failure(
                SessionErrorKind::Identity,
                format!("invalid persisted omp identity: {e}"),
            )
        })?;
        if header["type"] == "session" {
            return header["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    failure(
                        SessionErrorKind::Identity,
                        "omp session file has no persisted session identity",
                    )
                });
        }
        if header["type"] != "title" {
            break;
        }
    }
    Err(failure(
        SessionErrorKind::Identity,
        "omp session file has no persisted session identity",
    ))
}

/// get_state.sessionId is a provider-facing identity (it may change on /fresh).
/// The persisted session header is authoritative for restore/fork identity.
fn omp_state(
    launch: &[String],
    scope: &SessionScope,
    cwd: &Path,
    selector: &str,
    source: &str,
) -> Result<(PathBuf, String, Option<String>), SessionOperationError> {
    let mut command = Command::new(&launch[0]);
    command
        .args(&launch[1..])
        .args([selector, source, "--mode", "rpc"])
        .current_dir(cwd)
        .envs(&scope.environment);
    crate::core::proc::hide_console(&mut command);
    // Never attribute a background preparation process to its source pane.
    command
        .env_remove("LYREL")
        .env_remove(crate::daemon::pane::LYREL_PANE_ENV);
    let (output, timed_out) = rpc_output(&mut command).map_err(|e| {
        failure(
            SessionErrorKind::Command,
            format!("omp session preparation: {e}"),
        )
    })?;
    let state = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find(|frame| {
            frame["type"] == "response"
                && frame["id"] == "lyrel-session-state"
                && frame["command"] == "get_state"
        });
    let state = state.ok_or_else(|| {
        failure(
            SessionErrorKind::Command,
            if timed_out {
                "omp session preparation timed out before reporting identity".into()
            } else {
                format!(
                    "omp did not report session identity: {}",
                    String::from_utf8_lossy(&output.stderr)
                        .chars()
                        .take(1500)
                        .collect::<String>()
                )
            },
        )
    })?;
    if state["success"] != true {
        return Err(failure(
            SessionErrorKind::Command,
            state["error"].as_str().unwrap_or("omp get_state failed"),
        ));
    }
    let path = state["data"]["sessionFile"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "omp reported no persisted session file",
            )
        })?;
    let path = resolve(cwd, Path::new(path));
    let id = omp_identity(&path)?;
    let error = if timed_out {
        Some("omp created/reopened the session but shutdown timed out".into())
    } else if !output.status.success() {
        Some(format!(
            "omp reported identity but exited unsuccessfully ({})",
            output.status
        ))
    } else {
        None
    };
    Ok((path, id, error))
}

fn rpc_output(command: &mut Command) -> io::Result<(std::process::Output, bool)> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (state_tx, state_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        let mut frame = Vec::new();
        // v1 frames are at most 1 MiB; an uncooperative extension must not
        // allocate without bound or prevent a known identity from being kept.
        loop {
            frame.clear();
            let read = reader
                .by_ref()
                .take(1024 * 1024 + 1)
                .read_until(b'\n', &mut frame);
            if !matches!(read, Ok(n) if n > 0) || frame.len() > 1024 * 1024 {
                break;
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&frame)
                && value["type"] == "response"
                && value["id"] == "lyrel-session-state"
            {
                let _ = state_tx.send(std::mem::take(&mut frame));
                break;
            }
        }
        let _ = io::copy(&mut reader, &mut io::sink());
    });
    std::thread::spawn(move || {
        let mut reader = stderr;
        let mut bytes = Vec::new();
        let _ = reader.by_ref().take(64 * 1024).read_to_end(&mut bytes);
        let _ = stderr_tx.send(bytes);
        let _ = io::copy(&mut reader, &mut io::sink());
    });
    if let Err(error) = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"id\":\"lyrel-session-state\",\"type\":\"get_state\"}\n")
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    // EOF asks the native RPC runtime to drain and dispose. No prompt is sent.
    let deadline = Instant::now() + Duration::from_secs(45);
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                child.kill()?;
                break (child.wait()?, true);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };
    Ok((
        std::process::Output {
            status,
            stdout: state_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or_default(),
            stderr: stderr_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or_default(),
        },
        timed_out,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::local::LocalHost;

    #[test]
    fn scoped_launch_uses_target_shell_and_preserves_prepared_posix_resume() {
        let host = LocalHost::new();
        let line = "PI_IMAGE_PROTOCOL=none pi --session '/remote/My Sessions/source.jsonl'";
        let command = scoped_launch_command(&*host, line, Some("/bin/bash")).unwrap();
        assert_eq!(
            command,
            "PI_IMAGE_PROTOCOL=none pi --session '/remote/My Sessions/source.jsonl'"
        );
        assert_eq!(
            scoped_launch_command(&*host, &command, Some("/bin/bash")).unwrap(),
            command
        );
        let powershell = scoped_launch_command(&*host, line, Some("pwsh.exe")).unwrap();
        assert!(powershell.contains(
            "[Environment]::SetEnvironmentVariable('PI_IMAGE_PROTOCOL','none','Process')"
        ));
        assert!(powershell.contains("finally"));
    }

    #[test]
    fn scoped_launch_prefix_uses_last_assignment_and_rejects_unknown_selectors() {
        let host = LocalHost::new();
        assert_eq!(
            scoped_launch_command(
                &*host,
                "PI_FORCE_IMAGE_PROTOCOL=kitty PI_FORCE_IMAGE_PROTOCOL=none omp",
                Some("sh")
            )
            .unwrap(),
            "PI_FORCE_IMAGE_PROTOCOL=none omp"
        );
        assert!(
            scoped_launch_command(
                &*host,
                "PI_IMAGE_PROTOCOL=none SECRET=hidden pi",
                Some("sh")
            )
            .is_err()
        );
        assert!(scoped_launch_command(&*host, "PI_IMAGE_PROTOCOL=none", Some("sh")).is_err());
        assert_eq!(
            scoped_launch_command(&*host, "custom-wrapper pi", None).unwrap(),
            "custom-wrapper pi"
        );
    }

    #[test]
    fn omp_picture_policy_reaches_child_without_changing_parent_shell() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("policy.txt");
        let executable = std::env::current_exe().unwrap();
        let shell = if cfg!(windows) { "pwsh" } else { "sh" };
        let line = format!(
            "PI_FORCE_IMAGE_PROTOCOL=none PI_KITTY_PLACEHOLDERS=0 PI_NO_KITTY_PLACEHOLDERS=1 {}",
            [
                executable.to_string_lossy().into_owned(),
                "--ignored".into(),
                "--exact".into(),
                "host::session_operations::tests::omp_policy_child".into(),
                "--nocapture".into()
            ]
            .iter()
            .map(|arg| quote_for_shell(arg, Some(shell)))
            .collect::<Vec<_>>()
            .join(" ")
        );
        let child = scoped_launch_command(&*LocalHost::new(), &line, Some(shell)).unwrap();
        let line = if cfg!(windows) {
            format!(
                "{child}; [IO.File]::AppendAllText($env:LYREL_POLICY_OUTPUT, '|' + $env:PI_FORCE_IMAGE_PROTOCOL)"
            )
        } else {
            format!(
                "{child}; printf '|%s' \"$PI_FORCE_IMAGE_PROTOCOL\" >> \"$LYREL_POLICY_OUTPUT\""
            )
        };
        let mut command = Command::new(shell);
        if cfg!(windows) {
            command.args(["-NoProfile", "-Command"]);
        } else {
            command.arg("-c");
        }
        let status = command
            .arg(line)
            .env("LYREL_POLICY_OUTPUT", &output)
            .env("PI_FORCE_IMAGE_PROTOCOL", "parent-policy")
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            "none,0,1|parent-policy"
        );
    }

    #[test]
    #[ignore = "owned subprocess for public process-scoped launch behavior"]
    fn omp_policy_child() {
        let Some(output) = std::env::var_os("LYREL_POLICY_OUTPUT") else {
            return;
        };
        std::fs::write(
            output,
            format!(
                "{},{},{}",
                std::env::var("PI_FORCE_IMAGE_PROTOCOL").unwrap(),
                std::env::var("PI_KITTY_PLACEHOLDERS").unwrap(),
                std::env::var("PI_NO_KITTY_PLACEHOLDERS").unwrap(),
            ),
        )
        .unwrap();
    }

    fn operation(cwd: PathBuf) -> SessionOperation {
        SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Pi,
            kind: SessionOperationKind::Resume,
            source_id: "session-original".into(),
            cwd,
            launch_argv: vec!["pi".into(), "--model".into(), "provider/model".into()],
            scope: SessionScope::default(),
        }
    }

    fn persisted_pi_operation(cwd: PathBuf) -> SessionOperation {
        let mut request = operation(cwd.clone());
        let file = cwd.join("source.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"session\",\"version\":3,\"id\":\"session-original\"}\n",
        )
        .unwrap();
        request.scope.session_file = Some(file);
        request.scope.session_dir = Some(cwd);
        request
    }

    #[test]
    fn persisted_omp_launch_keeps_the_effective_store_and_profile() {
        let mut state = crate::core::cli_agent::AgentSessionState::default();
        state.launch_argv = Some(vec!["omp".into(), "--profile".into(), "old".into()]);
        state.readout.session_scope = Some(SessionScope {
            profile: Some("work".into()),
            session_dir: Some(PathBuf::from("/actual/store")),
            session_file: Some(PathBuf::from("/actual/store/target.jsonl")),
            environment: BTreeMap::from([("HOME".into(), "/actual/home".into())]),
            ..Default::default()
        });
        let context = launch_context(CLIAgent::OhMyPi, &state).unwrap();
        assert_eq!(
            context,
            [
                "HOME=/actual/home",
                "omp",
                "--profile",
                "old",
                "--profile",
                "work",
                "--session-dir",
                "/actual/store",
                "--resume",
                "/actual/store/target.jsonl"
            ]
        );
    }

    #[test]
    fn a_session_cannot_be_opened_on_a_different_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = operation(dir.path().into());
        request.expected_host = 42;
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::WrongHost);
        assert!(error.retained.is_none());
    }

    #[test]
    fn a_missing_directory_does_not_open_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let request = operation(dir.path().join("removed"));
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Directory);
    }

    #[test]
    fn resume_preserves_the_source_identity_and_launch_context() {
        let dir = tempfile::tempdir().unwrap();
        let request = persisted_pi_operation(dir.path().into());
        let prepared = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(prepared.target_id.as_deref(), Some("session-original"));
        assert_eq!(prepared.source_id, "session-original");
        assert_eq!(prepared.cwd, request.cwd);
        assert_eq!(prepared.session_file, request.scope.session_file);
        assert!(!prepared.created);
    }

    #[test]
    fn disabled_persistence_is_an_explicit_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = operation(dir.path().into());
        request.launch_argv.push("--no-session".into());
        assert_eq!(
            LocalHost::new()
                .session_operation(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Unsupported
        );
    }

    #[test]
    fn malformed_identity_cannot_become_shell_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = operation(dir.path().into());
        request.source_id = "id; destructive-command".into();
        assert_eq!(
            LocalHost::new()
                .session_operation(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Identity
        );
    }

    #[test]
    fn opening_failure_retains_the_same_identity_and_command_for_retry() {
        let dir = tempfile::tempdir().unwrap();
        let request = persisted_pi_operation(dir.path().into());
        let prepared = LocalHost::new().session_operation(&request).unwrap();
        let expected_command = prepared.command.clone();
        let error = prepared.opening_failed("pane connection refused");
        assert_eq!(error.kind, SessionErrorKind::Launch);
        let retained = error.retained.unwrap();
        assert_eq!(retained.target_id.as_deref(), Some("session-original"));
        assert_eq!(retained.command, expected_command);
        assert_eq!(retained.source_id, "session-original");
    }

    #[test]
    fn a_credential_argument_is_rejected_before_a_native_process_starts() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = operation(dir.path().into());
        request.agent = CLIAgent::OhMyPi;
        request.launch_argv = vec!["omp".into(), "--api-key=secret-must-not-appear".into()];
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Unsupported);
        assert!(!error.to_string().contains("secret-must-not-appear"));
    }

    fn persisted_omp_operation(cwd: PathBuf) -> SessionOperation {
        let mut request = persisted_pi_operation(cwd.clone());
        request.agent = CLIAgent::OhMyPi;
        request.launch_argv = vec![
            cwd.join("not-installed")
                .join(if cfg!(windows) { "omp.exe" } else { "omp" })
                .to_string_lossy()
                .into_owned(),
        ];
        request
    }

    #[test]
    fn omp_resume_reads_the_selected_identity_without_starting_its_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let request = persisted_omp_operation(dir.path().into());
        let source = request.scope.session_file.as_ref().unwrap();
        let before = std::fs::read(source).unwrap();
        let resumed = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(resumed.target_id.as_deref(), Some("session-original"));
        assert_eq!(resumed.session_file.as_ref(), Some(source));
        assert!(!resumed.created);
        assert_eq!(std::fs::read(source).unwrap(), before);

        let wrong = SessionOperation {
            source_id: "different-source".into(),
            ..request
        };
        assert_eq!(
            LocalHost::new().session_operation(&wrong).unwrap_err().kind,
            SessionErrorKind::Identity
        );
    }

    #[test]
    fn omp_id_lookup_is_not_limited_to_the_recent_history_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = persisted_omp_operation(dir.path().into());
        let source = request.scope.session_file.take().unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        for index in 0..501 {
            std::fs::write(
                dir.path().join(format!("newer-{index}.jsonl")),
                format!("{{\"type\":\"session\",\"id\":\"newer-{index}\"}}\n"),
            )
            .unwrap();
        }
        let resumed = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(resumed.target_id.as_deref(), Some("session-original"));
        assert_eq!(resumed.session_file.as_ref(), Some(&source));
        std::fs::copy(&source, dir.path().join("ambiguous.jsonl")).unwrap();
        assert_eq!(
            LocalHost::new()
                .session_operation(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Identity
        );
        request.source_id = "missing-source".into();
        assert_eq!(
            LocalHost::new()
                .session_operation(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Identity
        );
    }

    #[test]
    fn omp_id_lookup_keeps_the_selected_profile_and_agent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = persisted_omp_operation(dir.path().into());
        let source = request.scope.session_file.take().unwrap();
        request.scope.session_dir = None;
        request.scope.profile = Some("work".into());
        for key in SCOPE_ENV {
            request
                .scope
                .environment
                .insert((*key).into(), String::new());
        }
        let custom_agent = dir.path().join("custom-agent");
        let profile_store = dir.path().join(".omp/profiles/work/agent/sessions/project");
        let custom_store = custom_agent.join("sessions/project");
        for store in [&profile_store, &custom_store] {
            std::fs::create_dir_all(store).unwrap();
            std::fs::copy(&source, store.join("source.jsonl")).unwrap();
        }
        for key in ["HOME", "USERPROFILE"] {
            request
                .scope
                .environment
                .insert(key.into(), dir.path().to_string_lossy().into_owned());
        }
        request
            .scope
            .environment
            .insert("PI_CONFIG_DIR".into(), ".omp".into());
        request.scope.environment.insert(
            "PI_CODING_AGENT_DIR".into(),
            custom_agent.to_string_lossy().into_owned(),
        );
        let named = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(named.session_file, Some(profile_store.join("source.jsonl")));
        request.scope.profile = Some("default".into());
        let custom = LocalHost::new().session_operation(&request).unwrap();
        assert_eq!(custom.session_file, Some(custom_store.join("source.jsonl")));
    }

    #[test]
    fn omp_resume_rejects_a_source_outside_its_selected_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut request = persisted_omp_operation(dir.path().into());
        request.scope.session_dir = Some(store.path().into());
        assert_eq!(
            LocalHost::new()
                .session_operation(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Identity
        );
    }

    /// 仅使用显式指定的正式 OMP 安装；源、配置、扩展和 writer 都在临时目录。
    #[test]
    #[ignore = "requires LYREL_TEST_OMP pointing to a native OMP installation"]
    fn native_omp_fork_has_an_independent_reopenable_identity() {
        struct Owner(std::process::Child);
        impl Drop for Owner {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let binary = std::env::var("LYREL_TEST_OMP").unwrap();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        let store = root.path().join("sessions");
        for dir in [&home, &cwd, &store] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let source = store.join("parent.jsonl");
        let source_id = "native-active-parent";
        let records = [
            serde_json::json!({"type":"session","version":3,"id":source_id,"timestamp":"2026-10-26T00:00:00.000Z","cwd":cwd}),
            serde_json::json!({"type":"message","id":"fixture-user","parentId":null,"timestamp":"2026-10-26T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"离线回归夹具，不发送模型请求。"}],"timestamp":1792972801000_u64}}),
        ];
        std::fs::write(
            &source,
            records.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        let startup_log = root.path().join("startup.jsonl");
        let extension = root.path().join("write-on-start.ts");
        std::fs::write(
            &extension,
            format!(
                "import {{ appendFileSync }} from 'node:fs';\nexport default function(pi) {{ pi.on('session_start', (_event, ctx) => {{ pi.appendEntry('lyrel-active-writer-regression', {{pid:process.pid}}); appendFileSync({}, JSON.stringify({{session_id:ctx.sessionManager.getSessionId()}})+'\\n'); }}); pi.on('before_provider_request', () => {{ throw new Error('offline fixture forbids model calls'); }}); }}\n",
                serde_json::to_string(&startup_log).unwrap()
            ),
        )
        .unwrap();
        let mut request = operation(cwd.clone());
        request.agent = CLIAgent::OhMyPi;
        request.kind = SessionOperationKind::Fork;
        request.source_id = source_id.into();
        request.launch_argv = vec![
            binary,
            "--model".into(),
            "openai/gpt-4.1".into(),
            "--extension".into(),
            extension.to_string_lossy().into_owned(),
        ];
        request.scope.session_file = Some(source.clone());
        request.scope.session_dir = Some(store.clone());
        for key in SCOPE_ENV {
            request
                .scope
                .environment
                .insert((*key).into(), String::new());
        }
        for (key, value) in [
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
            ("OMP_PROFILE", "default".into()),
            ("PI_CONFIG_DIR", ".omp".into()),
            (
                "PI_CODING_AGENT_DIR",
                home.join(".omp/agent").to_string_lossy().into_owned(),
            ),
        ] {
            request.scope.environment.insert(key.into(), value);
        }
        let mut command = Command::new(&request.launch_argv[0]);
        command
            .args(&request.launch_argv[1..])
            .args([
                "--session-dir",
                store.to_str().unwrap(),
                "--resume",
                source.to_str().unwrap(),
                "--mode",
                "rpc",
            ])
            .current_dir(&cwd)
            .envs(&request.scope.environment)
            .env_remove("LYREL")
            .env_remove(crate::daemon::pane::LYREL_PANE_ENV)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::core::proc::hide_console(&mut command);
        let mut owner = Owner(command.spawn().unwrap());
        let stdout = owner.0.stdout.take().unwrap();
        let mut stderr = owner.0.stderr.take().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = io::BufReader::new(stdout);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
                if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line)
                    && frame["type"] == "response"
                    && frame["id"] == "lyrel-owner-ready"
                {
                    let _ = ready_tx.send(frame);
                    break;
                }
                line.clear();
            }
            let _ = io::copy(&mut reader, &mut io::sink());
        });
        std::thread::spawn(move || {
            let _ = io::copy(&mut stderr, &mut io::sink());
        });
        owner
            .0
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"{\"id\":\"lyrel-owner-ready\",\"type\":\"get_state\"}\n")
            .unwrap();
        let ready = ready_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(ready["success"], true);
        assert_eq!(ready["data"]["sessionFile"].as_str(), source.to_str());
        assert!(owner.0.try_wait().unwrap().is_none());
        let before = std::fs::read(&source).unwrap();

        let host = LocalHost::new();
        let fork = host.session_operation(&request).unwrap();
        assert!(fork.created);
        assert_ne!(fork.target_id.as_deref(), Some(source_id));
        let fork_contents = std::fs::read_to_string(fork.session_file.as_ref().unwrap()).unwrap();
        let fork_entries = fork_contents
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            fork_entries
                .iter()
                .find(|entry| entry["type"] == "session")
                .unwrap()["parentSession"],
            source_id
        );
        assert!(
            fork_entries
                .iter()
                .any(|entry| entry["type"] == "message" && entry["id"] == "fixture-user")
        );
        assert_eq!(std::fs::read(&source).unwrap(), before);
        let original = host
            .session_operation(&SessionOperation {
                kind: SessionOperationKind::Resume,
                ..request.clone()
            })
            .unwrap();
        assert_eq!(original.target_id.as_deref(), Some(source_id));
        request.kind = SessionOperationKind::Resume;
        request.source_id = fork.target_id.clone().unwrap();
        request.scope = fork.scope.clone();
        request.launch_argv = fork.launch_argv.clone();
        let reopened = host.session_operation(&request).unwrap();
        assert_eq!(reopened.target_id, fork.target_id);
        assert!(!reopened.created);
        let (file, id, error) = omp_state(
            &fork.launch_argv,
            &fork.scope,
            &fork.cwd,
            "--resume",
            fork.session_file.as_ref().unwrap().to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(Some(file), fork.session_file);
        assert_eq!(Some(id), fork.target_id);
        assert_eq!(error, None);
        assert!(owner.0.try_wait().unwrap().is_none());
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(&store)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
                .count(),
            2
        );
        let source_starts = std::fs::read_to_string(startup_log)
            .unwrap()
            .lines()
            .filter(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["session_id"] == source_id
            })
            .count();
        assert_eq!(source_starts, 1, "只读源定位不得再次执行源会话的启动扩展");
    }
}
