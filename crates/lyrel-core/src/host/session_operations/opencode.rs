//! OpenCode 2.x operations execute on the TUI's already authenticated client.

use super::*;
use crate::core::agent_history::PastSession;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeServer {
    pub pid: u64,
    pub urls: Vec<String>,
}

/// A per-TUI capability, never a provider credential or a persisted argv item.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeBridge {
    pub port: u16,
    pub client_id: String,
    pub pane: String,
    pub registration: PathBuf,
}

impl std::fmt::Debug for OpenCodeBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenCodeBridge")
            .field("client_id", &self.client_id)
            .field("pane", &self.pane)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeIdentity {
    pub id: String,
    pub project_id: String,
    pub created: u64,
    pub directory: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenCodeScope {
    pub connection_argv: Vec<String>,
    pub launch_argv: Vec<String>,
    pub server: Option<OpenCodeServer>,
    pub bridge: Option<OpenCodeBridge>,
    pub session: Option<OpenCodeIdentity>,
}

const MAX_REPLY: u64 = 8 * 1024 * 1024;

fn capability(bridge: &OpenCodeBridge) -> Result<String, SessionOperationError> {
    let bad = || {
        failure(
            SessionErrorKind::Connection,
            "OpenCode private client registration is unavailable or does not match",
        )
    };
    let dir = bridge.registration.parent().ok_or_else(bad)?;
    if bridge
        .registration
        .file_name()
        .is_none_or(|name| name != "client.json")
        || dir
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().starts_with("lyrel-opencode-"))
    {
        return Err(bad());
    }
    let dir_meta = std::fs::symlink_metadata(dir).map_err(|_| bad())?;
    let file_meta = std::fs::symlink_metadata(&bridge.registration).map_err(|_| bad())?;
    if !dir_meta.is_dir() || !file_meta.is_file() || file_meta.len() > 4096 {
        return Err(bad());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if dir_meta.uid() != unsafe { libc::geteuid() }
            || dir_meta.mode() & 0o077 != 0
            || file_meta.uid() != dir_meta.uid()
            || file_meta.mode() & 0o077 != 0
        {
            return Err(bad());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(bad)?
            .join("lyrel/opencode-clients");
        if dir.parent() != Some(root.as_path())
            || dir_meta.file_attributes() & 0x400 != 0
            || file_meta.file_attributes() & 0x400 != 0
        {
            return Err(bad());
        }
        let shell = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .ok_or_else(bad)?
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let script = "$ErrorActionPreference='Stop'; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value; foreach($p in @($env:LYREL_OPENCODE_PRIVATE_DIR,(Join-Path $env:LYREL_OPENCODE_PRIVATE_DIR 'client.json'))) { $a=Get-Acl -LiteralPath $p; if($a.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid) { exit 1 }; if($p -eq $env:LYREL_OPENCODE_PRIVATE_DIR -and -not $a.AreAccessRulesProtected) { exit 1 }; foreach($r in $a.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) { if($r.AccessControlType -eq 'Allow' -and $r.IdentityReference.Value -ne $sid) { exit 1 } } }";
        let mut command = Command::new(shell);
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("LYREL_OPENCODE_PRIVATE_DIR", dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::core::proc::hide_console(&mut command);
        let mut child = command.spawn().map_err(|_| bad())?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(bad());
                }
            }
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&bridge.registration).map_err(|_| bad())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata().map_err(|_| bad())?;
        if opened.ino() != file_meta.ino() || opened.dev() != file_meta.dev() {
            return Err(bad());
        }
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| bad())?;
    if bytes.len() > 4096 {
        return Err(bad());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if value["client_id"] != bridge.client_id
        || value["pane"] != bridge.pane
        || value["port"] != bridge.port
    {
        return Err(bad());
    }
    let token = value["token"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(bad)?;
    Ok(token.into())
}

// V2 authenticates both peers and every byte of the operation/response. A stale
// port owner sees only fresh challenges, never the private registration secret.
const BRIDGE_VERSION: u64 = 2;

fn hex32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn auth_mac(key: &[u8], domain: &str, parts: &[&str]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(b"lyrel-opencode-bridge-v2\0");
    mac.update(domain.as_bytes());
    mac.update(&[0]);
    for part in parts {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part.as_bytes());
    }
    mac
}

fn proof(key: &[u8], domain: &str, parts: &[&str]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = auth_mac(key, domain, parts).finalize().into_bytes();
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}

fn verify_proof(
    key: &[u8],
    domain: &str,
    parts: &[&str],
    supplied: &Value,
) -> Result<(), SessionOperationError> {
    let bytes = supplied.as_str().and_then(hex32).ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "OpenCode bridge authentication failed",
        )
    })?;
    auth_mac(key, domain, parts)
        .verify_slice(&bytes)
        .map_err(|_| {
            failure(
                SessionErrorKind::Identity,
                "OpenCode bridge authentication failed",
            )
        })
}

fn wire_write(stream: &mut TcpStream, value: &Value) -> Result<(), SessionOperationError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "cannot encode OpenCode client request",
        )
    })?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|_| {
        failure(
            SessionErrorKind::Connection,
            "OpenCode client request failed",
        )
    })
}

fn wire_read(
    reader: &mut io::BufReader<TcpStream>,
    deadline: Instant,
    limit: usize,
) -> Result<Value, SessionOperationError> {
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                failure(
                    SessionErrorKind::Connection,
                    "OpenCode client absolute deadline expired",
                )
            })?;
        reader
            .get_ref()
            .set_read_timeout(Some(remaining))
            .map_err(|_| {
                failure(
                    SessionErrorKind::Connection,
                    "cannot configure OpenCode client deadline",
                )
            })?;
        let buffer = reader.fill_buf().map_err(|_| {
            failure(
                SessionErrorKind::Connection,
                "OpenCode client did not answer before its deadline",
            )
        })?;
        if buffer.is_empty() {
            return Err(failure(
                SessionErrorKind::Connection,
                "OpenCode client disconnected",
            ));
        }
        let length = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(buffer.len());
        if bytes.len() + length > limit {
            return Err(failure(
                SessionErrorKind::Command,
                "OpenCode client frame exceeds its metadata limit",
            ));
        }
        let complete = buffer[length - 1] == b'\n';
        bytes.extend_from_slice(&buffer[..length]);
        reader.consume(length);
        if complete {
            return serde_json::from_slice(&bytes).map_err(|_| {
                failure(
                    SessionErrorKind::Command,
                    "invalid OpenCode client response",
                )
            });
        }
    }
}

fn bridge_call(
    scope: &OpenCodeScope,
    operation: &str,
    source: &str,
    cursor: Option<&str>,
) -> Result<Value, SessionOperationError> {
    let bridge = scope.bridge.as_ref().ok_or_else(|| failure(SessionErrorKind::Connection, "OpenCode TUI client is not connected; reopen the selected source in its native TUI before forking"))?;
    if bridge.port == 0 || bridge.client_id.is_empty() || bridge.pane.is_empty() {
        return Err(failure(
            SessionErrorKind::Identity,
            "invalid OpenCode client capability",
        ));
    }
    let key = hex32(&capability(bridge)?).ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "invalid OpenCode client capability",
        )
    })?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, bridge.port)),
        Duration::from_secs(2),
    )
    .map_err(|_| {
        failure(
            SessionErrorKind::Connection,
            "OpenCode source TUI client is disconnected",
        )
    })?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| {
            failure(
                SessionErrorKind::Connection,
                "cannot configure OpenCode client deadline",
            )
        })?;
    let client_nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    wire_write(
        &mut stream,
        &json!({"version": BRIDGE_VERSION, "client_id": bridge.client_id, "pane": bridge.pane, "client_nonce": client_nonce}),
    )?;
    let mut reader = io::BufReader::new(stream);
    let challenge = wire_read(&mut reader, Instant::now() + Duration::from_secs(2), 4096)?;
    let server_nonce = challenge["server_nonce"]
        .as_str()
        .filter(|s| hex32(s).is_some())
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "OpenCode bridge did not authenticate its server challenge",
            )
        })?;
    if challenge["version"] != BRIDGE_VERSION {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode bridge protocol is incompatible",
        ));
    }
    let transcript = [
        client_nonce.as_str(),
        server_nonce,
        bridge.client_id.as_str(),
        bridge.pane.as_str(),
    ];
    verify_proof(&key, "server-challenge", &transcript, &challenge["proof"])?;
    let payload =
        serde_json::to_string(&json!({"client_id": bridge.client_id, "pane": bridge.pane,
        "operation": operation, "source_id": source, "cursor": cursor, "server": scope.server}))
        .map_err(|_| {
            failure(
                SessionErrorKind::Command,
                "cannot encode OpenCode operation",
            )
        })?;
    let request_parts = [
        transcript[0],
        transcript[1],
        transcript[2],
        transcript[3],
        payload.as_str(),
    ];
    wire_write(
        reader.get_mut(),
        &json!({"version": BRIDGE_VERSION, "payload": payload, "proof": proof(&key, "request", &request_parts)}),
    )?;
    let reply = wire_read(&mut reader, Instant::now() + Duration::from_secs(25), MAX_REPLY as usize).map_err(|mut error| {
        if operation == "fork" { error.message = "OpenCode client did not confirm the fork; native creation may already have succeeded — inspect native history before repeating".into(); }
        error
    })?;
    if reply["version"] != BRIDGE_VERSION {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode bridge protocol is incompatible",
        ));
    }
    let response = reply["payload"].as_str().ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "OpenCode bridge response was not authenticated",
        )
    })?;
    verify_proof(
        &key,
        "response",
        &[
            transcript[0],
            transcript[1],
            transcript[2],
            transcript[3],
            &payload,
            response,
        ],
        &reply["proof"],
    )?;
    let value: Value = serde_json::from_str(response).map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "invalid authenticated OpenCode response",
        )
    })?;
    if value["client_id"] != bridge.client_id || value["pane"] != bridge.pane {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode response belongs to another client",
        ));
    }
    if value["ok"] != true {
        let kind = if value["kind"] == "identity" {
            SessionErrorKind::Identity
        } else {
            SessionErrorKind::Connection
        };
        let code = value["error"]
            .as_str()
            .filter(|s| {
                s.len() < 160
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b" -_:".contains(&b))
            })
            .unwrap_or("client operation failed");
        return Err(failure(kind, format!("OpenCode: {code}")));
    }
    Ok(value)
}

fn server(value: &Value) -> Result<OpenCodeServer, SessionOperationError> {
    let mut info: OpenCodeServer = serde_json::from_value(value.clone()).map_err(|_| {
        failure(
            SessionErrorKind::Identity,
            "OpenCode did not return a server identity",
        )
    })?;
    info.urls.sort();
    info.urls.dedup();
    if info.urls.is_empty() {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode returned no server connection URLs",
        ));
    }
    for value in &info.urls {
        let url: http::Uri = value.parse().map_err(|_| {
            failure(
                SessionErrorKind::Identity,
                "OpenCode returned an invalid service URL",
            )
        })?;
        if !matches!(url.scheme_str(), Some("http" | "https"))
            || url.authority().is_none_or(|a| a.as_str().contains('@'))
            || url.query().is_some()
            || value.contains('#')
        {
            return Err(failure(
                SessionErrorKind::Identity,
                "OpenCode returned a private or unsupported service URL",
            ));
        }
    }
    Ok(info)
}

fn info(
    value: &Value,
    expected: Option<&str>,
    cwd: Option<&Path>,
) -> Result<(String, PathBuf), SessionOperationError> {
    let id = value["id"]
        .as_str()
        .filter(|id| {
            id.starts_with("ses_") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "OpenCode returned no valid session ID",
            )
        })?;
    if expected.is_some_and(|expected| expected != id) {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode returned another session identity",
        ));
    }
    let directory = value["location"]["directory"].as_str().ok_or_else(|| {
        failure(
            SessionErrorKind::Directory,
            "OpenCode session has no working directory",
        )
    })?;
    let directory = PathBuf::from(directory);
    if cwd.is_some_and(|cwd| cwd != directory) {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode session belongs to another working directory",
        ));
    }
    Ok((id.into(), directory))
}

fn session_identity(value: &Value) -> Result<OpenCodeIdentity, SessionOperationError> {
    let (id, directory) = info(value, None, None)?;
    let project_id = value["projectID"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "OpenCode session has no native project identity",
            )
        })?;
    let created = value["time"]["created"].as_u64().ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "OpenCode session has no native creation identity",
        )
    })?;
    Ok(OpenCodeIdentity {
        id,
        directory,
        project_id: project_id.into(),
        created,
    })
}

fn launch(
    request: &SessionOperation,
    scope: &mut SessionScope,
) -> Result<Vec<String>, SessionOperationError> {
    if scope
        .environment
        .keys()
        .any(|key| !SCOPE_ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "OpenCode accepts only documented home/storage scope selectors",
        ));
    }
    let context = scope.opencode.as_ref().ok_or_else(|| {
        failure(
            SessionErrorKind::Identity,
            "OpenCode session has no verified service scope",
        )
    })?;
    let argv = if context.launch_argv.is_empty() {
        &request.launch_argv
    } else {
        &context.launch_argv
    };
    let mut start = 0;
    let mut launch_environment = BTreeMap::<String, String>::new();
    while let Some((key, value)) = argv.get(start).and_then(|arg| arg.split_once('=')) {
        if !SCOPE_ENV.contains(&key) {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "cannot replay an OpenCode credential or unknown environment assignment",
            ));
        }
        launch_environment.insert(key.into(), value.into());
        start += 1;
    }
    // Bound selectors win, then explicit launch assignments, then Host defaults.
    // Validate the final environment, never a pre-merge approximation of it.
    for (key, value) in launch_environment {
        scope.environment.entry(key).or_insert(value);
    }
    for key in SCOPE_ENV {
        if let Ok(value) = std::env::var(key) {
            scope.environment.entry((*key).into()).or_insert(value);
        }
    }
    if scope
        .environment
        .get("OPENCODE_DB")
        .is_some_and(|value| value == ":memory:")
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "the selected OpenCode configuration uses an in-memory store and cannot reopen a persisted session",
        ));
    }
    let executable = argv.get(start).ok_or_else(|| {
        failure(
            SessionErrorKind::Unsupported,
            "OpenCode executable is missing",
        )
    })?;
    if CLIAgent::detect_from_argv(&[executable.clone()]) != Some(CLIAgent::OpenCode) {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "cannot identify the captured OpenCode executable",
        ));
    }
    let mut out = vec![executable.clone()];
    let mut i = start + 1;
    while i < argv.len() {
        let arg = &argv[i];
        let key = arg.split('=').next().unwrap_or(arg);
        match key {
            "--session" | "-s" | "--prompt" | "--server" => {
                i += if arg.contains('=') { 1 } else { 2 }
            }
            "--continue" | "-c" | "--standalone" | "--fork" => i += 1,
            "--auto" | "--yolo" | "--dangerously-skip-permissions" | "--print-logs" => {
                out.push(arg.clone());
                i += 1;
            }
            "--" => break,
            _ if !arg.starts_with('-') => {
                i += 1;
            }
            _ => {
                return Err(failure(
                    SessionErrorKind::Unsupported,
                    "captured OpenCode option is not supported by the formal 2.x interactive CLI",
                ));
            }
        }
    }
    let connection = scope.opencode.as_ref().unwrap();
    if !connection.connection_argv.is_empty() && connection.connection_argv != ["--standalone"] {
        if connection.connection_argv.len() != 2 || connection.connection_argv[0] != "--server" {
            return Err(failure(
                SessionErrorKind::Identity,
                "invalid OpenCode connection selection",
            ));
        }
        let url: http::Uri = connection.connection_argv[1]
            .parse()
            .map_err(|_| failure(SessionErrorKind::Identity, "invalid OpenCode server URL"))?;
        if !matches!(url.scheme_str(), Some("http" | "https"))
            || url.authority().is_none_or(|a| a.as_str().contains('@'))
            || url.query().is_some()
            || connection.connection_argv[1].contains('#')
        {
            return Err(failure(
                SessionErrorKind::Unsupported,
                "OpenCode server URL must not contain credentials or private parameters",
            ));
        }
        out.extend(connection.connection_argv.iter().cloned());
    }
    Ok(out)
}

pub(super) fn prepare(
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    if request.kind == SessionOperationKind::Fork
        && request
            .scope
            .opencode
            .as_ref()
            .and_then(|s| s.bridge.as_ref())
            .is_none()
    {
        return Err(failure(
            SessionErrorKind::Connection,
            "OpenCode fork requires its bound foreground TUI client",
        ));
    }
    let mut scope = request.scope.clone();
    let launch = launch(request, &mut scope)?;
    let mut context = scope.opencode.clone().unwrap();
    if context.connection_argv == ["--standalone"] && context.bridge.is_none() {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "the original standalone TUI is disconnected; a new private service is not the selected source",
        ));
    }
    if context.bridge.is_none() && context.session.is_none() {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode restore has no saved native session identity; select the real session from scoped history",
        ));
    }
    let dialect = if context.bridge.is_some() {
        ApiDialect::Current
    } else {
        ApiDialect::discover(&launch, &scope, &request.cwd)?
    };
    let expected = match context.server.clone() {
        Some(server) => server,
        None if context.bridge.is_none() && request.kind == SessionOperationKind::Resume => {
            let identity = server(&native_api(
                &launch,
                &scope,
                &request.cwd,
                "server.info",
                &[],
                dialect,
            )?)?;
            context.server = Some(identity.clone());
            scope.opencode = Some(context.clone());
            identity
        }
        None => {
            return Err(failure(
                SessionErrorKind::Identity,
                "OpenCode live client did not report its service identity",
            ));
        }
    };
    let source = if context.bridge.is_some() {
        bridge_call(&context, "get", &request.source_id, None)?["data"].clone()
    } else {
        native_api(
            &launch,
            &scope,
            &request.cwd,
            "session.get",
            &[format!("sessionID={}", request.source_id)],
            dialect,
        )?["data"]
            .clone()
    };
    info(&source, Some(&request.source_id), Some(&request.cwd))?;
    let source_identity = session_identity(&source)?;
    if context
        .session
        .as_ref()
        .is_some_and(|expected| expected != &source_identity)
    {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode session metadata does not match the saved data source identity",
        ));
    }
    let standalone = context.connection_argv == ["--standalone"];
    if !standalone {
        verify_connection(&launch, &scope, &request.cwd, &expected, dialect)?;
    }
    let mut open = launch.clone();
    open.extend([
        "--session".into(),
        request.source_id.clone(),
        request.cwd.to_string_lossy().into_owned(),
    ]);
    command_line(&open, &scope)?;
    let created = request.kind == SessionOperationKind::Fork;
    let mut native_result_error = None;
    let target = if created {
        let reply = bridge_call(&context, "fork", &request.source_id, None)?;
        let id = reply["data"]["id"].as_str().filter(|id| id.starts_with("ses_") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .ok_or_else(|| failure(SessionErrorKind::Identity, "OpenCode did not confirm a valid fork identity; inspect native history before repeating"))?.to_owned();
        if id == request.source_id {
            return Err(failure(
                SessionErrorKind::Identity,
                "OpenCode did not create an independent fork",
            ));
        }
        match session_identity(&reply["data"]) {
            Ok(identity) if identity.directory == request.cwd => {
                context.session = Some(identity.clone());
                // Re-query the exact new ID through the original authenticated
                // client; a reported ID alone is not successful preparation.
                match bridge_call(&context, "get", &id, None).and_then(|verified| {
                    info(&verified["data"], Some(&id), Some(&request.cwd))?;
                    session_identity(&verified["data"])
                }) {
                    Ok(verified) if verified == identity => context.session = Some(verified),
                    Ok(_) => {
                        native_result_error = Some(failure(
                            SessionErrorKind::Identity,
                            "cannot verify created OpenCode target: native identity changed",
                        ))
                    }
                    Err(error) => {
                        native_result_error = Some(failure(
                            error.kind,
                            format!("cannot verify created OpenCode target: {}", error.message),
                        ))
                    }
                }
            }
            Ok(_) => {
                native_result_error = Some(failure(
                    SessionErrorKind::Identity,
                    "OpenCode created a fork in another directory",
                ))
            }
            Err(error) => native_result_error = Some(error),
        }
        id
    } else {
        context.session = Some(source_identity);
        request.source_id.clone()
    };
    context.launch_argv = launch.clone();
    scope.opencode = Some(context);
    open = launch.clone();
    open.extend([
        "--session".into(),
        target.clone(),
        request.cwd.to_string_lossy().into_owned(),
    ]);
    let command = command_line(&open, &scope);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: request.agent,
        source_id: request.source_id.clone(),
        target_id: Some(target),
        session_file: None,
        cwd: request.cwd.clone(),
        scope,
        launch_argv: launch,
        command: command.as_ref().ok().cloned(),
        created,
    };
    if let Some(error) = native_result_error {
        let kind = error.kind;
        let mut error = prepared.opening_failed(error.to_string());
        error.kind = kind;
        if let Some(retained) = &mut error.retained {
            retained.command = None;
        }
        return Err(error);
    }
    if standalone {
        let mut error = prepared.opening_failed("OpenCode created/retrieved the real target, but standalone keeps its private service credential inside the original TUI; formal 2.0.32 has no second-TUI authentication handoff");
        error.kind = SessionErrorKind::Unsupported;
        if let Some(retained) = &mut error.retained {
            retained.command = None;
        }
        return Err(error);
    }
    if let Err(error) = command {
        return Err(prepared.opening_failed(error.to_string()));
    }
    if let Err(error) = verify_connection(
        &prepared.launch_argv,
        &prepared.scope,
        &prepared.cwd,
        &expected,
        dialect,
    ) {
        let kind = error.kind;
        let mut error = prepared.opening_failed(error.to_string());
        error.kind = kind;
        return Err(error);
    }
    Ok(prepared)
}

fn verify_connection(
    launch: &[String],
    scope: &SessionScope,
    cwd: &Path,
    expected: &OpenCodeServer,
    dialect: ApiDialect,
) -> Result<(), SessionOperationError> {
    let actual = server(&native_api(
        launch,
        scope,
        cwd,
        "server.info",
        &[],
        dialect,
    )?)?;
    if &actual != expected {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode service connection changed; the selected session scope must be reconnected before opening",
        ));
    }
    Ok(())
}

/// A default scope is deliberately discovered only on the executing Host.
/// Bound clients never fall back to this path after disconnecting.
fn history_context(
    request: &SessionOperation,
) -> Result<(SessionScope, Vec<String>, OpenCodeScope, ApiDialect), SessionOperationError> {
    let mut scope = request.scope.clone();
    if scope.opencode.is_none() {
        let mut context = OpenCodeScope::default();
        let mut i = 0;
        while i < request.launch_argv.len() {
            let arg = &request.launch_argv[i];
            if arg == "--server" {
                i += 1;
                let value = request.launch_argv.get(i).ok_or_else(|| {
                    failure(
                        SessionErrorKind::Identity,
                        "configured OpenCode server URL is missing",
                    )
                })?;
                context
                    .connection_argv
                    .extend(["--server".into(), value.clone()]);
            } else if let Some(value) = arg.strip_prefix("--server=") {
                context
                    .connection_argv
                    .extend(["--server".into(), value.into()]);
            } else if arg == "--standalone" {
                context.connection_argv.push(arg.clone());
            }
            i += 1;
        }
        if context.connection_argv == ["--standalone"] {
            return Err(failure(
                SessionErrorKind::Connection,
                "standalone history requires its connected TUI client; a new private service is not the selected source",
            ));
        }
        scope.opencode = Some(context);
    }
    let launch = launch(request, &mut scope)?;
    let mut context = scope.opencode.clone().unwrap();
    let dialect = if context.bridge.is_some() {
        ApiDialect::Current
    } else {
        ApiDialect::discover(&launch, &scope, &request.cwd)?
    };
    let actual = if context.bridge.is_some() {
        server(&bridge_call(&context, "info", "", None)?["server"])?
    } else {
        server(&native_api(
            &launch,
            &scope,
            &request.cwd,
            "server.info",
            &[],
            dialect,
        )?)?
    };
    if context
        .server
        .as_ref()
        .is_some_and(|expected| expected != &actual)
    {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode history service does not match the selected scope",
        ));
    }
    context.server = Some(actual);
    context.launch_argv = launch.clone();
    scope.opencode = Some(context.clone());
    Ok((scope, launch, context, dialect))
}

pub(crate) fn sessions(
    request: &SessionOperation,
) -> Result<Vec<PastSession>, SessionOperationError> {
    let (scope, launch, context, dialect) = history_context(request)?;
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = std::collections::HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() >= deadline {
            return Err(failure(
                SessionErrorKind::Connection,
                "OpenCode history query exceeded its deadline",
            ));
        }
        let page = if context.bridge.is_some() {
            bridge_call(&context, "list", "", cursor.as_deref())?["data"].clone()
        } else {
            let mut params = vec![
                "limit=100".into(),
                "order=desc".into(),
                "parentID=null".into(),
            ];
            if let Some(cursor) = &cursor {
                params = vec!["limit=100".into(), format!("cursor={cursor}")];
            }
            native_api(
                &launch,
                &scope,
                &request.cwd,
                "session.list",
                &params,
                dialect,
            )?
        };
        let items = page["data"].as_array().ok_or_else(|| {
            failure(
                SessionErrorKind::Command,
                "OpenCode history response has no metadata list",
            )
        })?;
        for item in items {
            if !item["parentID"].is_null() || !item["time"]["archived"].is_null() {
                continue;
            }
            let (id, cwd) = info(item, None, None)?;
            let updated = item["time"]["updated"].as_u64().ok_or_else(|| {
                failure(
                    SessionErrorKind::Command,
                    "OpenCode history timestamp is invalid",
                )
            })? / 1000;
            let mut session_scope = scope.clone();
            session_scope.opencode.as_mut().unwrap().session = Some(session_identity(item)?);
            out.push(PastSession {
                agent: CLIAgent::OpenCode,
                id,
                cwd: Some(cwd),
                title: item["title"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("OpenCode")
                    .chars()
                    .take(120)
                    .collect(),
                branch: None,
                updated,
                scope: Some(session_scope),
            });
            if out.len() == 500 {
                break;
            }
        }
        cursor = page["cursor"]["next"].as_str().map(str::to_owned);
        if out.len() >= 500 || cursor.is_none() {
            break;
        }
        if !seen.insert(cursor.clone().unwrap()) {
            return Err(failure(
                SessionErrorKind::Command,
                "OpenCode history repeated its page cursor",
            ));
        }
    }
    if context.bridge.is_none() {
        verify_connection(
            &launch,
            &scope,
            &request.cwd,
            context.server.as_ref().unwrap(),
            dialect,
        )?;
    }
    Ok(out)
}

/// Metering projection only: no prompt, assistant text, tools or credentials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OpenCodeStatistics {
    pub identity: String,
    pub sessions: Vec<Value>,
    pub issues: Vec<String>,
}

struct MeterBytes(usize);
impl Write for MeterBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn statistics(
    request: &SessionOperation,
) -> Result<OpenCodeStatistics, SessionOperationError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let (scope, launch, context, dialect) = history_context(request)?;
    let expected = context.server.as_ref().unwrap();
    let identity = serde_json::to_string(&json!({"agent":"opencode","server":expected,
        "environment":scope.environment,"connection":context.connection_argv}))
    .map_err(|_| {
        failure(
            SessionErrorKind::Identity,
            "cannot identify OpenCode statistics scope",
        )
    })?;
    let query = |operation: &str,
                 session: &str,
                 cursor: Option<&str>|
     -> Result<Value, SessionOperationError> {
        if context.bridge.is_some() {
            return bridge_call(&context, operation, session, cursor).map(|v| v["data"].clone());
        }
        let (native, mut params) = match operation {
            "stats" => (
                "experimental.session.stats",
                vec!["timezone=UTC".into(), "tools=none".into()],
            ),
            "statistics-list" => ("session.list", vec!["limit=100".into()]),
            "statistics-messages" => (
                "session.message.list",
                vec![format!("sessionID={session}"), "limit=200".into()],
            ),
            _ => unreachable!(),
        };
        if let Some(cursor) = cursor {
            params.push(format!("cursor={cursor}"));
        } else if operation != "stats" {
            params.push("order=asc".into());
        }
        native_api(&launch, &scope, &request.cwd, native, &params, dialect)
    };
    let mut out = OpenCodeStatistics {
        identity,
        sessions: Vec::new(),
        issues: Vec::new(),
    };
    // The official aggregate is an independent coverage check, not added to message usage.
    let native_stats = match query("stats", "", None) {
        Ok(value) => Some(if value.get("data").is_some() {
            value["data"].clone()
        } else {
            value
        }),
        Err(error) => {
            out.issues
                .push(format!("OpenCode experimental.session.stats: {error}"));
            None
        }
    };
    let mut cursor = None;
    let mut cursors = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    let mut messages_count = 0usize;
    let mut bytes = 0usize;
    'sessions: loop {
        if Instant::now() >= deadline {
            out.issues
                .push("OpenCode statistics deadline reached".into());
            break;
        }
        let page = match query("statistics-list", "", cursor.as_deref()) {
            Ok(page) => page,
            Err(error) => {
                out.issues.push(error.to_string());
                break;
            }
        };
        let Some(items) = page["data"].as_array() else {
            out.issues
                .push("OpenCode statistics session page is invalid".into());
            break;
        };
        for item in items {
            let id = match session_identity(item) {
                Ok(identity) => identity.id,
                Err(error) => {
                    out.issues.push(error.to_string());
                    continue;
                }
            };
            if !ids.insert(id.clone()) {
                continue;
            }
            if out.sessions.len() >= 5000 {
                out.issues
                    .push("OpenCode statistics session window reached".into());
                break 'sessions;
            }
            let mut messages = Vec::new();
            let mut message_cursor = None;
            let mut message_cursors = std::collections::HashSet::new();
            let mut message_ids = std::collections::HashMap::new();
            loop {
                if Instant::now() >= deadline || messages_count >= 50000 || bytes >= 4 * 1024 * 1024
                {
                    out.issues.push(
                        "OpenCode statistics message/deadline/response window reached".into(),
                    );
                    break;
                }
                let page = match query("statistics-messages", &id, message_cursor.as_deref()) {
                    Ok(page) => page,
                    Err(error) => {
                        out.issues.push(format!("OpenCode {id}: {error}"));
                        break;
                    }
                };
                let Some(rows) = page["data"].as_array() else {
                    out.issues
                        .push(format!("OpenCode {id}: statistics message page is invalid"));
                    break;
                };
                for row in rows {
                    if row["type"] != "user"
                        && row["type"] != "assistant"
                        && row["type"] != "compaction"
                    {
                        continue;
                    }
                    let row = json!({"id":row["id"],"type":row["type"],"time":{"created":row["time"]["created"]},
                        "model":row["model"],"tokens":row["tokens"],"cost":row["cost"]});
                    let message_id = row["id"].as_str().ok_or_else(|| {
                        failure(
                            SessionErrorKind::Identity,
                            "OpenCode message identity missing",
                        )
                    })?;
                    if let Some(index) = message_ids.get(message_id) {
                        if messages[*index] != row {
                            out.issues.push(format!(
                                "OpenCode {id}: conflicting duplicate message identity"
                            ));
                        }
                        continue;
                    }
                    message_ids.insert(message_id.to_owned(), messages.len());
                    let mut count = MeterBytes(0);
                    serde_json::to_writer(&mut count, &row).expect("metering JSON serializes");
                    bytes += count.0;
                    messages.push(row);
                    messages_count += 1;
                }
                message_cursor = match next_statistics_cursor(&page) {
                    Ok(cursor) => cursor,
                    Err(error) => {
                        out.issues.push(error.to_string());
                        break;
                    }
                };
                let Some(next) = &message_cursor else {
                    break;
                };
                if !message_cursors.insert(next.clone()) {
                    out.issues
                        .push(format!("OpenCode {id}: repeated message cursor"));
                    break;
                }
            }
            out.sessions.push(json!({"info":{"id":id,"projectID":item["projectID"],"parentID":item["parentID"],"fork":item["fork"],"time":{"created":item["time"]["created"]}},"messages":messages}));
            if Instant::now() >= deadline || messages_count >= 50000 || bytes >= 4 * 1024 * 1024 {
                out.issues
                    .push("OpenCode statistics message/deadline/response window reached".into());
                break 'sessions;
            }
        }
        cursor = match next_statistics_cursor(&page) {
            Ok(cursor) => cursor,
            Err(error) => {
                out.issues.push(error.to_string());
                break;
            }
        };
        let Some(next) = &cursor else {
            break;
        };
        if !cursors.insert(next.clone()) {
            out.issues
                .push("OpenCode statistics repeated session cursor".into());
            break;
        }
    }
    if context.bridge.is_none() {
        verify_connection(&launch, &scope, &request.cwd, expected, dialect)?;
    } else {
        let actual = server(&bridge_call(&context, "info", "", None)?["server"])?;
        if &actual != expected {
            return Err(failure(
                SessionErrorKind::Identity,
                "OpenCode statistics service changed",
            ));
        }
    }
    if let Some(stats) = native_stats {
        let mut steps = 0u64;
        let mut tokens = [0u64; 5];
        let mut cost = 0f64;
        for session in &out.sessions {
            for row in session["messages"].as_array().into_iter().flatten() {
                if !session["info"]["fork"].is_null()
                    && row["time"]["created"].as_i64() < session["info"]["time"]["created"].as_i64()
                {
                    continue;
                }
                if row["type"] == "assistant" {
                    steps += 1;
                } else if row["type"] != "compaction" {
                    continue;
                }
                for (sum, value) in tokens.iter_mut().zip(native_token_values(&row["tokens"])) {
                    *sum = sum.checked_add(value.unwrap_or(0)).unwrap_or(u64::MAX);
                }
                cost += row["cost"].as_f64().unwrap_or(0.);
            }
        }
        if stats["steps"].as_u64() != Some(steps)
            || native_token_values(&stats["tokens"]) != tokens.map(Some)
            || !stats["cost"]
                .as_f64()
                .is_some_and(|native| (native - cost).abs() <= 1e-8 * (1. + native.abs()))
        {
            out.issues.push("OpenCode native aggregate and message metering differ; history changed, compaction events are unavailable, or pagination is incomplete".into());
        }
    }
    Ok(out)
}

fn native_token_values(tokens: &Value) -> [Option<u64>; 5] {
    [
        tokens["input"].as_u64(),
        tokens["output"].as_u64(),
        tokens["reasoning"].as_u64(),
        tokens["cache"]["read"].as_u64(),
        tokens["cache"]["write"].as_u64(),
    ]
}

fn next_statistics_cursor(page: &Value) -> Result<Option<String>, SessionOperationError> {
    if !page["cursor"].is_object() {
        return Err(failure(
            SessionErrorKind::Command,
            "OpenCode statistics pagination metadata is missing",
        ));
    }
    match &page["cursor"]["next"] {
        Value::Null => Ok(None),
        Value::String(value) if !value.is_empty() && value.len() <= 65536 => {
            Ok(Some(value.clone()))
        }
        _ => Err(failure(
            SessionErrorKind::Command,
            "OpenCode statistics cursor is invalid",
        )),
    }
}

#[derive(Clone, Copy)]
enum ApiDialect {
    Current,
    InitialV2,
}

impl ApiDialect {
    fn discover(
        launch: &[String],
        scope: &SessionScope,
        cwd: &Path,
    ) -> Result<Self, SessionOperationError> {
        // The selected CLI owns discovery/authentication. Never probe a different URL.
        let schema = api(launch, scope, cwd, &["GET", "/openapi.json"], &[])?;
        if schema["paths"]["/api/info"]["get"]["operationId"] == "server.info" {
            return Ok(Self::Current);
        }
        if schema["paths"]["/api/health"]["get"]["operationId"] == "v2.health.get"
            && schema["paths"]["/api/server"]["get"]["operationId"] == "v2.server.get"
            && schema["paths"]["/api/session"]["get"]["operationId"] == "v2.session.list"
        {
            return Ok(Self::InitialV2);
        }
        Err(failure(
            SessionErrorKind::Unsupported,
            "The selected OpenCode service exposes no supported public history/identity API; check its version and native service connection",
        ))
    }
}

fn initial_v2_identity(
    mut query: impl FnMut(&str) -> Result<Value, SessionOperationError>,
) -> Result<Value, SessionOperationError> {
    // Early 2.x splits the same identity across health and server endpoints.
    // Guard both sides of the URL read against a service restart or replacement.
    let before = query("v2.health.get")?;
    let mut address = query("v2.server.get")?;
    let mut after = query("v2.health.get")?;
    let stable = before["healthy"] == true
        && after["healthy"] == true
        && before["pid"].as_u64().is_some_and(|pid| pid > 0)
        && before["pid"] == after["pid"]
        && before["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
        && before["version"] == after["version"];
    if !stable {
        return Err(failure(
            SessionErrorKind::Identity,
            "OpenCode service identity changed while reading its native metadata",
        ));
    }
    Ok(json!({"pid":after["pid"].take(),"urls":address["urls"].take()}))
}

fn native_api(
    launch: &[String],
    scope: &SessionScope,
    cwd: &Path,
    operation: &str,
    params: &[String],
    dialect: ApiDialect,
) -> Result<Value, SessionOperationError> {
    let operation = match dialect {
        ApiDialect::Current => operation,
        ApiDialect::InitialV2 => match operation {
            "server.info" => {
                return initial_v2_identity(|operation| api(launch, scope, cwd, &[operation], &[]));
            }
            "session.get" => "v2.session.get",
            "session.list" => "v2.session.list",
            "session.message.list" => "v2.message.list",
            "experimental.session.stats" => "v2.session.stats",
            _ => {
                return Err(failure(
                    SessionErrorKind::Unsupported,
                    "Unsupported OpenCode public API operation",
                ));
            }
        },
    };
    api(launch, scope, cwd, &[operation], params)
}

fn api(
    launch: &[String],
    scope: &SessionScope,
    cwd: &Path,
    operation: &[&str],
    params: &[String],
) -> Result<Value, SessionOperationError> {
    let mut command = Command::new(&launch[0]);
    command.arg("api").args(operation);
    if let Some(context) = &scope.opencode {
        command.args(&context.connection_argv);
    }
    for param in params {
        command.args(["--param", param]);
    }
    command
        .current_dir(cwd)
        .envs(&scope.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
        .env_remove("LYREL")
        .env_remove(crate::daemon::pane::LYREL_PANE_ENV);
    crate::core::proc::hide_console(&mut command);
    let mut child = command.spawn().map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "cannot start the captured OpenCode API CLI",
        )
    })?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        let mut bytes = Vec::new();
        let result = reader.by_ref().take(MAX_REPLY + 1).read_to_end(&mut bytes);
        let _ = tx.send(result.map(|_| bytes));
        let _ = io::copy(&mut reader, &mut io::sink());
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failure(
                    SessionErrorKind::Connection,
                    "OpenCode API CLI did not complete before its deadline",
                ));
            }
        }
    };
    let output = rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| {
            failure(
                SessionErrorKind::Command,
                "OpenCode API CLI did not close its response",
            )
        })?
        .map_err(|_| {
            failure(
                SessionErrorKind::Command,
                "cannot read OpenCode API response",
            )
        })?;
    if output.len() as u64 > MAX_REPLY {
        return Err(failure(
            SessionErrorKind::Command,
            "OpenCode API metadata exceeds its response limit",
        ));
    }
    if !status.success() {
        let kind = if serde_json::from_slice::<Value>(&output)
            .ok()
            .is_some_and(|v| v["_tag"] == "SessionNotFoundError")
        {
            SessionErrorKind::Identity
        } else {
            SessionErrorKind::Connection
        };
        return Err(failure(
            kind,
            "OpenCode API rejected the request; verify the selected session and service connection in its native TUI",
        ));
    }
    serde_json::from_slice(&output).map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "OpenCode API CLI returned invalid JSON",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::local::LocalHost;

    #[test]
    fn early_v2_identity_rejects_a_service_restart_between_metadata_reads() {
        for final_pid in [42, 43] {
            let mut replies = std::collections::VecDeque::from([
                json!({"healthy":true,"version":"2.0.3","pid":42}),
                json!({"urls":["http://127.0.0.1:12345/"]}),
                json!({"healthy":true,"version":"2.0.3","pid":final_pid}),
            ]);
            let identity = initial_v2_identity(|_| Ok(replies.pop_front().unwrap()));
            if final_pid == 42 {
                let identity = server(&identity.unwrap()).unwrap();
                assert_eq!(identity.pid, 42);
                assert_eq!(identity.urls, ["http://127.0.0.1:12345/"]);
            } else {
                assert_eq!(identity.unwrap_err().kind, SessionErrorKind::Identity);
            }
        }
    }

    fn request(cwd: &Path) -> SessionOperation {
        SessionOperation {
            expected_host: 0,
            agent: CLIAgent::OpenCode,
            kind: SessionOperationKind::Fork,
            source_id: "ses_source".into(),
            cwd: cwd.into(),
            launch_argv: vec!["opencode".into()],
            scope: SessionScope::default(),
        }
    }

    #[test]
    fn opencode_fork_without_a_bound_client_cannot_fork_another_default_service() {
        let dir = tempfile::tempdir().unwrap();
        let error = LocalHost::new()
            .session_operation(&request(dir.path()))
            .unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Connection);
        assert!(error.retained.is_none());
    }

    #[test]
    fn opencode_bridge_registration_does_not_expose_a_capability() {
        let bridge = OpenCodeBridge {
            port: 12345,
            client_id: "client-a".into(),
            pane: "42".into(),
            registration: PathBuf::from("private-client-registration"),
        };
        assert!(!serde_json::to_string(&bridge).unwrap().contains("token"));
    }

    #[test]
    fn opencode_resume_rejects_an_unknown_scope_before_starting_a_tui() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        request.kind = SessionOperationKind::Resume;
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Identity);
        assert!(error.retained.is_none());
    }

    #[test]
    fn opencode_untrusted_registration_cannot_be_used_as_a_session_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        request.scope.opencode = Some(OpenCodeScope {
            server: Some(OpenCodeServer {
                pid: 1,
                urls: vec!["http://127.0.0.1:1/".into()],
            }),
            bridge: Some(OpenCodeBridge {
                port: 1,
                client_id: "client".into(),
                pane: "42".into(),
                registration: dir.path().join("other.json"),
            }),
            ..Default::default()
        });
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Connection);
        assert!(error.retained.is_none());
    }

    #[test]
    fn durable_scopes_roundtrip_for_every_supported_agent_without_a_client_lease() {
        use crate::core::machine::AgentFacts;
        let scope = SessionScope {
            profile: Some("work".into()),
            session_dir: Some(PathBuf::from("/store")),
            session_file: Some(PathBuf::from("/store/source.jsonl")),
            shell_program: Some("bash".into()),
            environment: BTreeMap::from([
                ("HOME".into(), "/isolated-home".into()),
                ("OPENCODE_PASSWORD".into(), "must-not-persist".into()),
            ]),
            opencode: Some(OpenCodeScope {
                bridge: Some(OpenCodeBridge {
                    port: 1,
                    client_id: "view".into(),
                    pane: "42".into(),
                    registration: PathBuf::from("/private/client.json"),
                }),
                ..Default::default()
            }),
        };
        for agent in CLIAgent::ALL {
            let facts = AgentFacts {
                agent,
                session_id: Some("same-id".into()),
                launch_argv: None,
                session_scope: Some(scope.clone()),
                status: None,
            };
            let serialized = serde_json::to_string(&facts).unwrap();
            assert!(!serialized.contains("must-not-persist"));
            assert!(!serialized.contains("/private/client.json"));
            let restored: AgentFacts = serde_json::from_str(&serialized).unwrap();
            let restored = restored.session_scope.unwrap();
            assert_eq!(restored.profile.as_deref(), Some("work"));
            assert_eq!(
                restored.session_file.as_deref(),
                Some(Path::new("/store/source.jsonl"))
            );
            assert_eq!(
                restored.environment.get("HOME").map(String::as_str),
                Some("/isolated-home")
            );
            assert!(restored.opencode.unwrap().bridge.is_none());
        }
    }

    #[cfg(unix)]
    struct ClientFixture {
        _directory: tempfile::TempDir,
        scope: OpenCodeScope,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        received_secret: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    #[cfg(unix)]
    impl ClientFixture {
        fn new(cwd: &Path, fork_error: bool) -> Self {
            Self::with_behavior(cwd, fork_error, "none")
        }

        fn with_attack(cwd: &Path, attack: &'static str) -> Self {
            Self::with_behavior(cwd, false, attack)
        }

        fn with_behavior(cwd: &Path, fork_error: bool, attack: &'static str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let directory = tempfile::Builder::new()
                .prefix("lyrel-opencode-")
                .tempdir()
                .unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let registration = directory.path().join("client.json");
            let token = "a".repeat(64);
            std::fs::write(
                &registration,
                serde_json::to_vec(
                    &json!({"client_id":"client", "pane":"42", "port":port, "token":token}),
                )
                .unwrap(),
            )
            .unwrap();
            std::fs::set_permissions(&registration, std::fs::Permissions::from_mode(0o600))
                .unwrap();
            let identity = OpenCodeServer {
                pid: 123,
                urls: vec![format!("http://127.0.0.1:{port}/")],
            };
            let scope = OpenCodeScope {
                connection_argv: vec!["--standalone".into()],
                launch_argv: vec!["opencode".into()],
                server: Some(identity.clone()),
                bridge: Some(OpenCodeBridge {
                    port,
                    client_id: "client".into(),
                    pane: "42".into(),
                    registration,
                }),
                session: None,
            };
            let cwd = cwd.to_string_lossy().into_owned();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopping = stop.clone();
            let received_secret = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let secret_seen = received_secret.clone();
            let thread = std::thread::spawn(move || {
                let key = hex32(&token).unwrap();
                let mut saved_response: Option<Value> = None;
                while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(_) => break,
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    if line.contains(&token) {
                        secret_seen.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    let Ok(hello) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let client_nonce = hello["client_nonce"].as_str().unwrap_or("");
                    let server_nonce = format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    );
                    let transcript = [client_nonce, server_nonce.as_str(), "client", "42"];
                    let challenge_proof = if attack == "unsigned-challenge" {
                        "0".repeat(64)
                    } else {
                        proof(&key, "server-challenge", &transcript)
                    };
                    if attack == "dripping-challenge" {
                        for _ in 0..60 {
                            if stream.write_all(b" ").is_err() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        continue;
                    }
                    if writeln!(
                        stream,
                        "{}",
                        json!({"version":2,"server_nonce":server_nonce,"proof":challenge_proof})
                    )
                    .is_err()
                    {
                        continue;
                    }
                    line.clear();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    if line.contains(&token) {
                        secret_seen.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let Some(payload) = frame["payload"].as_str() else {
                        continue;
                    };
                    if frame["version"] != 2
                        || verify_proof(
                            &key,
                            "request",
                            &[
                                transcript[0],
                                transcript[1],
                                transcript[2],
                                transcript[3],
                                payload,
                            ],
                            &frame["proof"],
                        )
                        .is_err()
                    {
                        continue;
                    }
                    let Ok(request) = serde_json::from_str::<Value>(payload) else {
                        continue;
                    };
                    let session = |id: &str, title: &str| json!({"id":id,"projectID":"proj_fixture","location":{"directory":cwd},"title":title,"time":{"created":1690000000000_u64,"updated":1700000000123_u64}});
                    let data = match request["operation"].as_str() {
                        Some("info") => Value::Null,
                        Some("get") => session(request["source_id"].as_str().unwrap(), "Source"),
                        Some("fork") => session("ses_nativefork", "Independent fork"),
                        Some("list") => {
                            let mut child = session("ses_child", "Subagent");
                            child["parentID"] = "ses_source".into();
                            let mut archived = session("ses_archived", "Archived");
                            archived["time"]["archived"] = 1700000000123_u64.into();
                            let mut fork = session("ses_nativefork", "Independent fork");
                            fork["fork"] = json!({"sessionID":"ses_source"});
                            json!({"data":[session("ses_source","Source"),child,archived,fork],"cursor":{}})
                        }
                        _ => continue,
                    };
                    let reply = if request["operation"] == "fork" && fork_error {
                        json!({"ok":false,"kind":"identity","error":"foreground session changed","client_id":"client","pane":"42"})
                    } else if request["operation"] == "get"
                        && request["source_id"] == "ses_nativefork"
                        && attack == "missing-created-target"
                    {
                        json!({"ok":false,"kind":"identity","error":"native target does not exist","client_id":"client","pane":"42"})
                    } else {
                        json!({"ok":true,"client_id":"client","pane":"42","server":identity,"data":data})
                    };
                    let response_payload = reply.to_string();
                    let mac = proof(
                        &key,
                        "response",
                        &[
                            transcript[0],
                            transcript[1],
                            transcript[2],
                            transcript[3],
                            payload,
                            &response_payload,
                        ],
                    );
                    let mut response = json!({"version":2,"payload":response_payload,"proof":mac});
                    if attack == "modified-response" {
                        response["payload"] = json!({"ok":true,"client_id":"client","pane":"42","data":{"id":"ses_forged"}}).to_string().into();
                    }
                    if attack == "replayed-response" {
                        if let Some(saved) = &saved_response {
                            response = saved.clone();
                        } else {
                            saved_response = Some(response.clone());
                        }
                    }
                    let _ = writeln!(stream, "{response}");
                }
            });
            Self {
                _directory: directory,
                scope,
                stop,
                received_secret,
                thread: Some(thread),
            }
        }
    }

    #[cfg(unix)]
    impl Drop for ClientFixture {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn opencode_history_keeps_native_forks_but_excludes_subagents_and_archives() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::new(cwd.path(), false);
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        let sessions = LocalHost::new().opencode_sessions(&request).unwrap();
        assert_eq!(
            sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["ses_source", "ses_nativefork"]
        );
        assert_eq!(sessions[1].title, "Independent fork");
        assert_eq!(sessions[0].cwd.as_deref(), Some(cwd.path()));
        assert_eq!(sessions[0].updated, 1_700_000_000);
        let other_client = ClientFixture::new(cwd.path(), false);
        request.scope.opencode = Some(other_client.scope.clone());
        let other = LocalHost::new().opencode_sessions(&request).unwrap();
        assert_eq!(sessions[0].id, other[0].id);
        assert_ne!(sessions[0].scope, other[0].scope);
    }

    #[cfg(unix)]
    #[test]
    fn opencode_foreground_change_cannot_fork_a_background_tab() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::new(cwd.path(), true);
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Identity);
        assert!(error.retained.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn opencode_same_id_and_directory_cannot_replace_saved_native_project_identity() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::new(cwd.path(), false);
        let mut request = request(cwd.path());
        let mut scope = client.scope.clone();
        scope.session = Some(OpenCodeIdentity {
            id: "ses_source".into(),
            directory: cwd.path().into(),
            project_id: "another-storage-project".into(),
            created: 1_690_000_000_000,
        });
        request.scope.opencode = Some(scope);
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Identity);
        assert!(error.retained.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn opencode_native_creation_is_retained_when_second_tui_authentication_is_unavailable() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::new(cwd.path(), false);
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        let retained = error.retained.unwrap();
        assert!(retained.created);
        assert_eq!(retained.target_id.as_deref(), Some("ses_nativefork"));
        assert_eq!(retained.source_id, "ses_source");
        assert!(retained.command.is_none());
        request.source_id = retained.target_id.clone().unwrap();
        request.kind = SessionOperationKind::Resume;
        let reopened = LocalHost::new()
            .session_operation(&request)
            .unwrap_err()
            .retained
            .unwrap();
        assert_eq!(reopened.target_id, retained.target_id);
        assert!(!reopened.created);
    }

    #[cfg(unix)]
    #[test]
    fn opencode_disconnected_client_never_falls_back_to_default_history() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::new(cwd.path(), false);
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        drop(client);
        assert_eq!(
            LocalHost::new()
                .opencode_sessions(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Connection
        );
    }

    #[cfg(unix)]
    #[test]
    fn opencode_stale_port_owner_cannot_learn_secret_or_supply_history() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::with_attack(cwd.path(), "unsigned-challenge");
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        let error = LocalHost::new().opencode_sessions(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Identity);
        assert!(error.retained.is_none());
        assert!(
            !client
                .received_secret
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }

    #[cfg(unix)]
    #[test]
    fn opencode_modified_or_replayed_responses_are_not_accepted_as_native_metadata() {
        for attack in ["modified-response", "replayed-response"] {
            let cwd = tempfile::tempdir().unwrap();
            let client = ClientFixture::with_attack(cwd.path(), attack);
            let mut request = request(cwd.path());
            request.scope.opencode = Some(client.scope.clone());
            assert_eq!(
                LocalHost::new()
                    .opencode_sessions(&request)
                    .unwrap_err()
                    .kind,
                SessionErrorKind::Identity
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn opencode_created_target_survives_failed_native_get_verification() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::with_attack(cwd.path(), "missing-created-target");
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        let error = LocalHost::new().session_operation(&request).unwrap_err();
        let retained = error.retained.unwrap();
        assert_eq!(retained.target_id.as_deref(), Some("ses_nativefork"));
        assert!(retained.created);
        assert!(retained.command.is_none());
        assert!(error.message.contains("verify"));
    }

    #[cfg(unix)]
    #[test]
    fn opencode_effective_scope_prefers_bound_scope_then_launch_environment_then_host() {
        use std::os::unix::fs::PermissionsExt;
        let cwd = tempfile::tempdir().unwrap();
        let executable = cwd.path().join("opencode");
        // External CLI boundary: returned titles make the effective process
        // environment observable through real Host history metadata.
        std::fs::write(&executable, "#!/bin/sh\ncase \"$2\" in\nGET) printf '%s\\n' '{\"paths\":{\"/api/info\":{\"get\":{\"operationId\":\"server.info\"}}}}' ;;\nserver.info) printf '%s\\n' '{\"pid\":55,\"urls\":[\"http://127.0.0.1:12345/\"]}' ;;\nsession.list) printf '{\"data\":[{\"id\":\"ses_fixture\",\"projectID\":\"proj_fixture\",\"title\":\"%s\",\"location\":{\"directory\":\"%s\"},\"time\":{\"created\":1690000000000,\"updated\":1700000000000}}],\"cursor\":{}}\\n' \"$HOME\" \"$PWD\" ;;\nesac\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut request = request(cwd.path());
        request.launch_argv = vec![
            "HOME=/explicit-launch-home".into(),
            "OPENCODE_DB=/explicit-launch-store.db".into(),
            executable.to_string_lossy().into_owned(),
        ];
        let found = LocalHost::new().opencode_sessions(&request).unwrap();
        assert_eq!(found[0].title, "/explicit-launch-home");
        assert_eq!(
            found[0]
                .scope
                .as_ref()
                .unwrap()
                .environment
                .get("OPENCODE_DB")
                .map(String::as_str),
            Some("/explicit-launch-store.db")
        );
        request
            .scope
            .environment
            .insert("HOME".into(), "/bound-home".into());
        assert_eq!(
            LocalHost::new().opencode_sessions(&request).unwrap()[0].title,
            "/bound-home"
        );
        request.launch_argv[1] = "OPENCODE_DB=:memory:".into();
        assert_eq!(
            LocalHost::new()
                .opencode_sessions(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Unsupported
        );
        request
            .scope
            .environment
            .insert("OPENCODE_DB".into(), "/bound-store.db".into());
        assert_eq!(
            LocalHost::new().opencode_sessions(&request).unwrap()[0].title,
            "/bound-home"
        );
    }

    #[cfg(unix)]
    #[test]
    fn opencode_slow_peer_cannot_extend_the_absolute_authentication_deadline() {
        let cwd = tempfile::tempdir().unwrap();
        let client = ClientFixture::with_attack(cwd.path(), "dripping-challenge");
        let mut request = request(cwd.path());
        request.scope.opencode = Some(client.scope.clone());
        let started = Instant::now();
        assert_eq!(
            LocalHost::new()
                .opencode_sessions(&request)
                .unwrap_err()
                .kind,
            SessionErrorKind::Connection
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "traffic must not renew authentication time"
        );
    }

    #[test]
    #[ignore = "requires an isolated real OpenCode TUI bridge and LYREL_TEST_OPENCODE_SCOPE/CWD/SESSION"]
    fn native_opencode_unauthenticated_drip_does_not_starve_authenticated_history() {
        let cwd = PathBuf::from(std::env::var("LYREL_TEST_OPENCODE_CWD").unwrap());
        let mut request = request(&cwd);
        request.scope =
            serde_json::from_str(&std::env::var("LYREL_TEST_OPENCODE_SCOPE").unwrap()).unwrap();
        let port = request
            .scope
            .opencode
            .as_ref()
            .unwrap()
            .bridge
            .as_ref()
            .unwrap()
            .port;
        let source = std::env::var("LYREL_TEST_OPENCODE_SESSION").unwrap();
        let mut pending = Vec::new();
        for _ in 0..64 {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket.write_all(b"{").unwrap();
            pending.push(socket);
        }
        let sessions = LocalHost::new().opencode_sessions(&request).unwrap();
        assert!(sessions.iter().any(|session| session.id == source));
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            for socket in &mut pending {
                let _ = socket.write_all(b" ");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for mut socket in pending {
            socket
                .set_read_timeout(Some(Duration::from_millis(300)))
                .unwrap();
            match socket.read(&mut [0; 1]) {
                Ok(0) => {}
                Err(error)
                    if !matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) => {}
                other => {
                    panic!("unauthenticated connection exceeded its accept deadline: {other:?}")
                }
            }
        }
    }

    #[test]
    #[ignore = "requires isolated real daemon endpoint/pane/session with an actual failed foreground OpenCode execution and installed TUI plugin"]
    fn native_opencode_plugin_reports_failed_outcome_from_formal_sdk_envelopes() {
        use crate::core::cli_agent::{AgentOutcome, AgentStatus};
        use crate::daemon::protocol::{DaemonMsg, WinSize};
        let endpoint = std::env::var("LYREL_TEST_OPENCODE_DAEMON").unwrap();
        let pane = std::env::var("LYREL_TEST_OPENCODE_PANE")
            .unwrap()
            .parse()
            .unwrap();
        let source = std::env::var("LYREL_TEST_OPENCODE_SESSION").unwrap();
        let client = crate::client::PaneClient::at(endpoint);
        let mut observation = client
            .observe(
                pane,
                WinSize {
                    cols: 100,
                    rows: 30,
                    cell_w: 8,
                    cell_h: 16,
                },
            )
            .unwrap();
        observation
            .set_recv_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                Instant::now() < deadline,
                "real OpenCode TUI did not report a failed terminal outcome"
            );
            if let DaemonMsg::AgentStatus(Some(state)) = observation.recv().unwrap() {
                if state.session_id.as_deref() != Some(source.as_str()) {
                    continue;
                }
                if state
                    .readout
                    .result
                    .as_ref()
                    .is_some_and(|r| r.outcome == AgentOutcome::Failed)
                {
                    assert_eq!(state.status, AgentStatus::Done);
                    break;
                }
            }
        }
    }

    #[test]
    #[ignore = "requires isolated LYREL_TEST_OPENCODE_SCOPE JSON, LYREL_TEST_OPENCODE_CWD and a foreground LYREL_TEST_OPENCODE_SESSION in the real TUI"]
    fn native_opencode_fork_has_an_independent_reopenable_identity() {
        let cwd = PathBuf::from(std::env::var("LYREL_TEST_OPENCODE_CWD").unwrap());
        let mut request = request(&cwd);
        request.scope =
            serde_json::from_str(&std::env::var("LYREL_TEST_OPENCODE_SCOPE").unwrap()).unwrap();
        request.source_id = std::env::var("LYREL_TEST_OPENCODE_SESSION").unwrap();
        if !request
            .scope
            .opencode
            .as_ref()
            .unwrap()
            .launch_argv
            .is_empty()
        {
            request.launch_argv = request.scope.opencode.as_ref().unwrap().launch_argv.clone();
        } else if let Ok(binary) = std::env::var("LYREL_TEST_OPENCODE_BINARY") {
            request.launch_argv = vec![binary];
        }
        let original_scope = request.scope.clone();
        let host = LocalHost::new();
        let prepared = host.session_operation(&request).unwrap();
        assert!(prepared.created);
        assert_ne!(prepared.target_id.as_ref().unwrap(), &request.source_id);
        let retained = prepared
            .clone()
            .opening_failed("new pane disconnected")
            .retained
            .unwrap();
        request.kind = SessionOperationKind::Resume;
        request.source_id = retained.target_id.clone().unwrap();
        request.scope = retained.scope.clone();
        let reopened = host.session_operation(&request).unwrap();
        assert_eq!(reopened.target_id, prepared.target_id);
        assert!(!reopened.created);
        request.source_id = prepared.source_id;
        request.scope = original_scope;
        assert_eq!(
            host.session_operation(&request).unwrap().target_id.as_ref(),
            Some(&request.source_id)
        );
    }
}
