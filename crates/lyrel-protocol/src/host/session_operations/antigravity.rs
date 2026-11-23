//! Antigravity CLI (`agy`) stored conversations: read-only history discovery
//! and exact resume (#385).
//!
//! Verified against `antigravity-cli` 1.5.15 on Windows, including its own
//! `--help` contract:
//!
//! - App data lives in `<home>/.gemini/antigravity-cli`. The desktop
//!   Antigravity 2.0 app (`antigravity`) and the IDE (`antigravity-ide`) keep
//!   separate stores, which this adapter never merges into the CLI's.
//! - The searchable conversation index is `conversation_summaries.db`. Its
//!   `search_schema_version` table currently holds `3`; a different version
//!   is an unknown format and is reported instead of guessed at.
//! - `conversation_summaries` names the conversation id, title, preview,
//!   workspace URIs, modification time, parent conversation and app data
//!   directory. Root CLI conversations have no parent and depth `0`; nested
//!   subagent conversations are not resumable and are not listed.
//! - One stored conversation is `conversations/<id>.db`; its transcript is
//!   `brain/<id>/.system_generated/logs/transcript.jsonl`. The transcript path
//!   is the exact file the observer hook also reports for the live session,
//!   so a history row and an already-open pane can be compared by it.
//! - `agy --conversation <id>` resumes one exact conversation. `--continue`
//!   and `-c` select the most recent one and are removed from replayed launch
//!   arguments; print mode is never substituted for the interactive TUI.
//!
//! Reading goes through the Host's read-only SQLite browser and its agent
//! environment. Both are existing, negotiated Host capabilities
//! (`sqlite-browse-v1`, `agent-environment`), so a remote peer that predates
//! them refuses with an upgrade diagnostic while every other history source
//! keeps working; no provider-specific control variant is introduced. Exact
//! resume itself executes on the owning Host and is gated by the
//! `antigravity-v1` feature in `RemoteHost::session_operation`.

use super::*;
use crate::core::agent_history::PastSession;

/// The CLI's directory under `<home>/.gemini`.
const APP_DATA_DIR: &str = "antigravity-cli";
const INDEX_FILE: &str = "conversation_summaries.db";
const INDEX_TABLE: &str = "conversation_summaries";
const VERSION_TABLE: &str = "search_schema_version";
/// The observed index schema. Anything else is an unknown store format.
const INDEX_SCHEMA_VERSION: i64 = 3;
/// Columns this adapter reads. A renamed or missing column is an unknown
/// format, not an empty history.
const INDEX_COLUMNS: &[&str] = &[
    "conversation_id",
    "title",
    "preview",
    "last_modified_time",
    "workspace_uris",
    "parent_conversation_id",
    "nesting_depth",
    "app_data_dir",
];
const TITLE_CHARS: usize = 120;
/// Index reads are local or one bounded control round trip; five seconds is
/// the browser default, ten leaves room for a busy WAL.
const QUERY_SECONDS: u64 = 10;
/// Home selectors an Antigravity scope may replay. Everything else is either
/// a provider-specific store selector this adapter does not own or a
/// credential-shaped variable that must not enter a command line.
const ENV: &[&str] = &["HOME", "USERPROFILE"];

/// Values an interactive Antigravity launch can carry into an exact resume.
/// `--settings` is deliberately absent: its JSON can carry provider settings
/// that are not this adapter's to replay.
const VALUE_OPTIONS: &[&str] = &[
    "--model",
    "--effort",
    "--mode",
    "--agent",
    "--project",
    "--add-dir",
    "--log-file",
];
const SWITCH_OPTIONS: &[&str] = &["--dangerously-skip-permissions", "--sandbox"];
/// Selectors and one-shot modes that name a different conversation or a
/// non-interactive run. They are dropped from the replayed launch rather than
/// turning a resume into a second selection or a print request.
const STALE_ACTIONS: &[&str] = &[
    "--conversation",
    "--continue",
    "-c",
    "--prompt-interactive",
    "-i",
    "--prompt",
    "--print",
    "-p",
    "--input-format",
    "--output-format",
    "--json-schema",
    "--print-timeout",
    "--disable-slash-commands",
    "--new-project",
    "--remote-agent",
    "--remote-control",
];
/// Stale actions that always consume the following value.
const ACTIONS_WITH_VALUE: &[&str] = &[
    "--conversation",
    "--prompt-interactive",
    "-i",
    "--input-format",
    "--output-format",
    "--json-schema",
    "--print-timeout",
];
/// Print-mode aliases consume a following non-option token as their prompt.
const OPTIONAL_PROMPT_ACTIONS: &[&str] = &["--prompt", "--print", "-p"];

/// One raw row from the index, before it is trusted as a conversation. A
/// `None` means the cell was not a complete text/integer value: an unknown
/// shape for that row, never an empty title or workspace.
#[derive(Clone, Debug)]
struct RawSummary {
    id: Option<String>,
    title: Option<String>,
    preview: Option<String>,
    updated: Option<String>,
    workspace_uris: Option<String>,
    parent: Option<String>,
    nesting: Option<i64>,
    app_data_dir: Option<String>,
}

/// A conversation the exact-resume path re-read from the index.
#[derive(Clone, Debug)]
struct StoredSummary {
    title: Option<String>,
    cwd: PathBuf,
}

/// The three history entry points (Session History, Search Everywhere and
/// Sessions settings) call this through the owning Host. It never lists
/// another app's store and never writes.
pub fn sessions<H: Host + ?Sized>(
    host: &H,
    directory: Option<&Path>,
    request: &SessionOperation,
) -> Result<Vec<PastSession>, SessionOperationError> {
    if request.agent != CLIAgent::Antigravity {
        return Err(failure(
            SessionErrorKind::Identity,
            "Antigravity history requires an Antigravity launch context",
        ));
    }
    if request.expected_host != host.id().0 {
        return Err(failure(
            SessionErrorKind::WrongHost,
            "Antigravity history belongs to another Host",
        ));
    }
    if !host.is_connected() {
        return Err(failure(
            SessionErrorKind::Connection,
            "Antigravity history Host is disconnected",
        ));
    }
    let root = store_root(host, &request.scope)?;
    let Some(rows) = read_rows(host, &root, "")? else {
        // No index yet: a legitimate empty store, not an unknown format.
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    let mut unreadable = 0usize;
    for raw in rows {
        match raw.app_data_dir.as_deref() {
            Some(APP_DATA_DIR) => {}
            // The desktop app and IDE keep their own stores; `agy` cannot
            // resume those conversations.
            Some(_) => continue,
            None => {
                unreadable += 1;
                continue;
            }
        }
        match raw.parent.as_deref() {
            Some("") => {}
            // A subagent conversation is not a resumable root conversation.
            Some(_) => continue,
            None => {
                unreadable += 1;
                continue;
            }
        }
        match raw.nesting {
            Some(0) => {}
            Some(_) => continue,
            None => {
                unreadable += 1;
                continue;
            }
        }
        let Some(session) = session_from_row(host, &root, &raw) else {
            unreadable += 1;
            continue;
        };
        if directory.is_some_and(|dir| {
            !session.cwd.as_deref().is_some_and(|cwd| {
                crate::core::agent_history::directory_matches_with(cwd, dir, host.separator())
            })
        }) {
            continue;
        }
        found.push(session);
    }
    if found.is_empty() && unreadable > 0 {
        return Err(failure(
            SessionErrorKind::Unsupported,
            format!(
                "the Antigravity conversation index has {unreadable} conversation(s) this build cannot read exactly; no conversation for this directory was confirmed"
            ),
        ));
    }
    Ok(found)
}

/// Prepare one exact resume on the owning Host. The index is re-read now, so
/// a row deleted or moved since the listing is refused instead of launching a
/// nearby conversation. Fork is refused before any store access: 1.5.15 has
/// no non-interactive fork entry, and the documented interactive `/fork`
/// switches the invoking terminal instead of leaving an independent source
/// pane (#386).
pub(super) fn prepare<H: Host + ?Sized>(
    host: &H,
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    if request.kind == SessionOperationKind::Fork {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Antigravity has no independent fork route: /fork switches the invoking terminal; run /fork inside the agy pane instead",
        ));
    }
    validate_scope(request)?;
    let launch = launch_arguments(&request.launch_argv)?;
    let root = store_root(host, &request.scope)?;
    let stored = find_summary(host, &root, &request.source_id)?;
    validate_cwd(host, &stored.cwd, &request.cwd)?;
    let conversation = conversation_path(host, &root, &request.source_id);
    if !host.stat(&conversation).is_ok_and(|meta| !meta.is_dir) {
        return Err(failure(
            SessionErrorKind::Identity,
            format!(
                "selected Antigravity conversation store is missing: {}",
                conversation.display()
            ),
        ));
    }
    let transcript = transcript_path(host, &root, &request.source_id);
    if let Some(selected) = &request.scope.session_file {
        let same = |a: &Path, b: &Path| {
            crate::core::agent_history::directory_matches_with(a, b, host.separator())
        };
        if !same(selected, &transcript) && !same(selected, &conversation) {
            return Err(failure(
                SessionErrorKind::Identity,
                "the selected Antigravity file does not belong to this conversation",
            ));
        }
    }
    let mut scope = request.scope.clone();
    scope.session_file = Some(transcript.clone());
    let mut open = launch.clone();
    open.extend(["--conversation".to_string(), request.source_id.clone()]);
    let command = command_line(&open, &scope);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: request.agent,
        source_id: request.source_id.clone(),
        target_id: Some(request.source_id.clone()),
        session_file: Some(transcript),
        title: stored.title,
        cwd: request.cwd.clone(),
        scope,
        launch_argv: launch,
        command: command.as_ref().ok().cloned(),
        created: false,
    };
    match command {
        Ok(_) => Ok(prepared),
        Err(error) => Err(prepared.opening_failed(error.to_string())),
    }
}

fn validate_scope(request: &SessionOperation) -> Result<(), SessionOperationError> {
    if request.scope.profile.is_some()
        || request.scope.session_dir.is_some()
        || request.scope.opencode.is_some()
        || request
            .scope
            .environment
            .keys()
            .any(|key| !ENV.contains(&key.as_str()) && !TERMINAL_SCOPE_ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Antigravity scope accepts only home and public terminal environment selectors",
        ));
    }
    Ok(())
}

/// The effective `<home>/.gemini/antigravity-cli`. The scope's home wins
/// (what the pane observed), then the owning Host's own environment. Nothing
/// is ever read from the GUI's machine for a remote Host.
fn store_root<H: Host + ?Sized>(
    host: &H,
    scope: &SessionScope,
) -> Result<PathBuf, SessionOperationError> {
    let keys = if host.separator() == '\\' {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    let from_scope = keys
        .iter()
        .find_map(|key| scope.environment.get(*key))
        .filter(|value| !value.is_empty());
    let from_host = || {
        host.agent_environment().ok().and_then(|environment| {
            keys.iter()
                .find_map(|key| environment.get(*key))
                .filter(|value| !value.is_empty())
                .cloned()
        })
    };
    let home = from_scope
        .cloned()
        .or_else(from_host)
        .map(PathBuf::from)
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Directory,
                "Antigravity history requires the owning Host's home directory; this Host did not report one (update lyrel-server if it predates agent-environment)",
            )
        })?;
    if !host.is_absolute(&home) {
        return Err(failure(
            SessionErrorKind::Directory,
            "Antigravity home must be an absolute path on its owning Host",
        ));
    }
    Ok(host.join(&host.join(&home, ".gemini"), APP_DATA_DIR))
}

fn conversation_path<H: Host + ?Sized>(host: &H, root: &Path, id: &str) -> PathBuf {
    host.join(&host.join(root, "conversations"), &format!("{id}.db"))
}

fn transcript_path<H: Host + ?Sized>(host: &H, root: &Path, id: &str) -> PathBuf {
    host.join(
        &host.join(
            &host.join(
                &host.join(&host.join(root, "brain"), id),
                ".system_generated",
            ),
            "logs",
        ),
        "transcript.jsonl",
    )
}

/// Re-read the exact index row and validate the directory it recorded. A
/// missing row, a different directory, a subagent row or a desktop-app row
/// are all identity failures: none of them is the requested conversation.
fn find_summary<H: Host + ?Sized>(
    host: &H,
    root: &Path,
    id: &str,
) -> Result<StoredSummary, SessionOperationError> {
    let Some(rows) = read_rows(host, root, id)? else {
        return Err(failure(
            SessionErrorKind::Identity,
            "selected Antigravity conversation no longer exists in this store",
        ));
    };
    let mut matches = rows.into_iter().filter(|row| row.id.as_deref() == Some(id));
    let Some(raw) = matches.next() else {
        return Err(failure(
            SessionErrorKind::Identity,
            "selected Antigravity conversation no longer exists in this store",
        ));
    };
    if matches.next().is_some() {
        return Err(failure(
            SessionErrorKind::Identity,
            "Antigravity conversation identity is ambiguous in this store",
        ));
    }
    match raw.app_data_dir.as_deref() {
        Some(APP_DATA_DIR) => {}
        Some(_) => {
            return Err(failure(
                SessionErrorKind::Identity,
                "this conversation belongs to another Antigravity app data store, not the CLI",
            ));
        }
        None => {
            return Err(failure(
                SessionErrorKind::Identity,
                "the Antigravity conversation row has no readable app data directory",
            ));
        }
    }
    if raw
        .parent
        .as_deref()
        .is_some_and(|parent| !parent.is_empty())
        || raw.nesting.is_some_and(|depth| depth > 0)
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Antigravity subagent conversations cannot be resumed as a root conversation",
        ));
    }
    if raw.parent.is_none() || raw.nesting.is_none() {
        return Err(failure(
            SessionErrorKind::Identity,
            "the Antigravity conversation row has no readable parent/depth identity",
        ));
    }
    let cwd = raw
        .workspace_uris
        .as_deref()
        .and_then(primary_workspace_path)
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Identity,
                "the Antigravity conversation has no readable workspace directory",
            )
        })?;
    let title = display_title(&raw);
    Ok(StoredSummary {
        title: Some(title),
        cwd,
    })
}

fn validate_cwd<H: Host + ?Sized>(
    host: &H,
    stored: &Path,
    requested: &Path,
) -> Result<(), SessionOperationError> {
    let stored = host.canonicalize(stored).map_err(|error| {
        failure(
            SessionErrorKind::Directory,
            format!("the Antigravity conversation workspace is unavailable: {error}"),
        )
    })?;
    let requested = host
        .canonicalize(requested)
        .map_err(|error| failure(SessionErrorKind::Directory, error.to_string()))?;
    if !crate::core::agent_history::directory_matches_with(&stored, &requested, host.separator()) {
        return Err(failure(
            SessionErrorKind::Identity,
            "the Antigravity conversation workspace differs from the selected directory",
        ));
    }
    Ok(())
}

fn session_from_row<H: Host + ?Sized>(
    host: &H,
    root: &Path,
    raw: &RawSummary,
) -> Option<PastSession> {
    let id = raw.id.as_deref().filter(|id| valid_id(id))?;
    let cwd = primary_workspace_path(raw.workspace_uris.as_deref()?)?;
    let updated = parse_time(raw.updated.as_deref()?)?;
    let mut scope = SessionScope::default();
    scope.session_file = Some(transcript_path(host, root, id));
    Some(PastSession {
        agent: CLIAgent::Antigravity,
        id: id.to_string(),
        cwd: Some(cwd),
        title: display_title(raw),
        branch: None,
        updated,
        scope: Some(scope),
    })
}

fn display_title(raw: &RawSummary) -> String {
    raw.title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            raw.preview
                .as_deref()
                .map(str::trim)
                .filter(|preview| !preview.is_empty())
        })
        .map(|text| text.chars().take(TITLE_CHARS).collect())
        .or_else(|| raw.id.clone())
        .unwrap_or_default()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

/// Open the index, check its version and columns, and read the rows matching
/// `search`. `Ok(None)` is a store that has never been created; every other
/// readability problem is an `Unsupported` diagnostic.
fn read_rows<H: Host + ?Sized>(
    host: &H,
    root: &Path,
    search: &str,
) -> Result<Option<Vec<RawSummary>>, SessionOperationError> {
    let path = host.join(root, INDEX_FILE);
    let cancel = crate::host::sqlite::SqliteCancel::new();
    let opened = match host.sqlite_open(&path, &cancel) {
        Ok(opened) => opened,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(failure(
                SessionErrorKind::Unsupported,
                format!(
                    "the Antigravity conversation index {} cannot be read read-only: {error}",
                    path.display()
                ),
            ));
        }
    };
    let result = read_index(host, opened.view, &cancel, search);
    host.sqlite_close(opened.view);
    result.map(Some)
}

fn read_index<H: Host + ?Sized>(
    host: &H,
    view: u64,
    cancel: &crate::host::sqlite::SqliteCancel,
    search: &str,
) -> Result<Vec<RawSummary>, SessionOperationError> {
    let version_columns = host
        .sqlite_columns(view, VERSION_TABLE, cancel)
        .map_err(|error| unknown_index(&error))?;
    if !version_columns.names.iter().any(|name| name == "version") {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "the Antigravity conversation index has no readable schema version",
        ));
    }
    let version_page = host
        .sqlite_page(
            view,
            &crate::host::sqlite::SqliteQuery {
                table: VERSION_TABLE.into(),
                columns: vec!["version".into()],
                order: Vec::new(),
                search: String::new(),
                sort: None,
                page: 0,
                start_column: 0,
                seconds: QUERY_SECONDS,
            },
            cancel,
        )
        .map_err(|error| unknown_index(&error))?;
    let version = version_page
        .rows
        .first()
        .and_then(|row| row.first())
        .and_then(cell_i64)
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Unsupported,
                "the Antigravity conversation index has no readable schema version",
            )
        })?;
    if version != INDEX_SCHEMA_VERSION {
        return Err(failure(
            SessionErrorKind::Unsupported,
            format!(
                "the Antigravity conversation index schema v{version} is not supported by this build (verified v{INDEX_SCHEMA_VERSION}); update Lyrel"
            ),
        ));
    }
    let columns = host
        .sqlite_columns(view, INDEX_TABLE, cancel)
        .map_err(|error| unknown_index(&error))?;
    let missing: Vec<&str> = INDEX_COLUMNS
        .iter()
        .copied()
        .filter(|expected| !columns.names.iter().any(|name| name == expected))
        .collect();
    if !missing.is_empty() {
        return Err(failure(
            SessionErrorKind::Unsupported,
            format!(
                "the Antigravity conversation index is missing column(s) {}; this build cannot list it exactly",
                missing.join(", ")
            ),
        ));
    }
    let query = crate::host::sqlite::SqliteQuery {
        table: INDEX_TABLE.into(),
        columns: INDEX_COLUMNS
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        order: Vec::new(),
        search: search.to_string(),
        // `last_modified_time` is the fourth requested column.
        sort: Some((3, true)),
        page: 0,
        start_column: 0,
        seconds: QUERY_SECONDS,
    };
    let page = host
        .sqlite_page(view, &query, cancel)
        .map_err(|error| unknown_index(&error))?;
    Ok(page.rows.into_iter().map(raw_summary).collect())
}

fn unknown_index(error: &io::Error) -> SessionOperationError {
    failure(
        SessionErrorKind::Unsupported,
        format!("the Antigravity conversation index is not a format this build can read: {error}"),
    )
}

fn raw_summary(values: Vec<crate::host::sqlite::SqliteValue>) -> RawSummary {
    let cell = |index: usize| values.get(index);
    RawSummary {
        id: cell_text(cell(0)),
        title: cell_text(cell(1)),
        preview: cell_text(cell(2)),
        updated: cell_text(cell(3)),
        workspace_uris: cell_text(cell(4)),
        parent: cell_text(cell(5)),
        nesting: cell(6).and_then(cell_i64),
        app_data_dir: cell_text(cell(7)),
    }
}

fn cell_text(value: Option<&crate::host::sqlite::SqliteValue>) -> Option<String> {
    match value? {
        crate::host::sqlite::SqliteValue::Text { preview, total }
            if preview.len() as u64 == *total =>
        {
            String::from_utf8(preview.clone()).ok()
        }
        _ => None,
    }
}

fn cell_i64(value: &crate::host::sqlite::SqliteValue) -> Option<i64> {
    match value {
        crate::host::sqlite::SqliteValue::Integer { value } => Some(*value),
        _ => None,
    }
}

/// The first `file://` workspace URI, decoded. The first entry is the
/// conversation's primary working directory; `--add-dir` extras are not the
/// directory a resume is scoped to.
fn primary_workspace_path(uris: &str) -> Option<PathBuf> {
    let list: Vec<String> = serde_json::from_str(uris).ok()?;
    list.iter().find_map(|uri| file_uri_path(uri))
}

fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(rest)?;
    if decoded.is_empty() {
        return None;
    }
    let bytes = decoded.as_bytes();
    if bytes.first() == Some(&b'/')
        && bytes.get(1).is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.get(2) == Some(&b':')
    {
        // A Windows drive URI (`file:///C:/repo`) has a POSIX-looking leading
        // slash that is not part of the path.
        return Some(PathBuf::from(&decoded[1..]));
    }
    Some(PathBuf::from(decoded))
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// SQLite stores `last_modified_time` as `YYYY-MM-DD HH:MM:SS[.fraction]`
/// with a UTC or numeric offset. The engine returns it as text, so the
/// conversion stays here instead of pulling a date crate into the protocol.
fn parse_time(text: &str) -> Option<u64> {
    let (date, clock) = text.trim().split_once(' ')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset_minutes) = match clock.rfind(['+', '-']) {
        Some(position) if position >= 8 => {
            let offset = &clock[position..];
            let sign = if offset.starts_with('-') { -1i64 } else { 1 };
            let mut fields = offset[1..].split(':');
            let hours: i64 = fields.next()?.parse().ok()?;
            let minutes: i64 = fields.next()?.parse().ok()?;
            (&clock[..position], sign * (hours * 60 + minutes))
        }
        _ => (clock, 0),
    };
    let clock = clock.strip_suffix('Z').unwrap_or(clock);
    let mut fields = clock.split(':');
    let hour: i64 = fields.next()?.parse().ok()?;
    let minute: i64 = fields.next()?.parse().ok()?;
    let second: i64 = fields.next()?.split('.').next()?.parse().ok()?;
    if fields.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    u64::try_from(seconds).ok().filter(|seconds| *seconds > 0)
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The replayed interactive launch, with every stale selector and one-shot
/// mode removed. Unknown options fail closed: guessing could start a
/// different conversation or a non-interactive run.
fn launch_arguments(argv: &[String]) -> Result<Vec<String>, SessionOperationError> {
    let fallback = ["agy".to_string()];
    let argv = if argv.is_empty() {
        fallback.as_slice()
    } else {
        argv
    };
    let named = (0..argv.len())
        .find(|index| CLIAgent::detect_from_argv(&argv[..=*index]) == Some(CLIAgent::Antigravity))
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Unsupported,
                "cannot identify the Antigravity executable in the configured launch",
            )
        })?;
    let mut launch = argv[..=named].to_vec();
    let mut index = named + 1;
    while index < argv.len() {
        let argument = &argv[index];
        let key = argument.split('=').next().unwrap_or(argument);
        if STALE_ACTIONS.contains(&key) {
            index += 1;
            if ACTIONS_WITH_VALUE.contains(&key) && !argument.contains('=') {
                if argv.get(index).is_some_and(|value| !value.starts_with('-')) {
                    index += 1;
                }
            } else if OPTIONAL_PROMPT_ACTIONS.contains(&key)
                && !argument.contains('=')
                && argv.get(index).is_some_and(|value| !value.starts_with('-'))
            {
                index += 1;
            }
            continue;
        }
        if VALUE_OPTIONS.contains(&key) {
            launch.push(argument.clone());
            if !argument.contains('=') {
                index += 1;
                launch.push(
                    argv.get(index)
                        .ok_or_else(|| {
                            failure(
                                SessionErrorKind::Unsupported,
                                format!("incomplete Antigravity launch option {argument}"),
                            )
                        })?
                        .clone(),
                );
            }
            index += 1;
            continue;
        }
        if SWITCH_OPTIONS.contains(&key) && !argument.contains('=') {
            launch.push(argument.clone());
            index += 1;
            continue;
        }
        return Err(failure(
            SessionErrorKind::Unsupported,
            format!(
                "Antigravity launch option {argument} cannot be replayed for an exact resume; remove it from agent_launch.antigravity or start the conversation manually"
            ),
        ));
    }
    Ok(launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_uris_decode_posix_and_windows_file_urls() {
        assert_eq!(
            file_uri_path("file:///home/me/repo"),
            Some(PathBuf::from("/home/me/repo"))
        );
        assert_eq!(
            file_uri_path("file:///C:/Users/me/Repo"),
            Some(PathBuf::from("C:/Users/me/Repo"))
        );
        assert_eq!(
            file_uri_path("file:///c%3A/Users/me/My%20Repo"),
            Some(PathBuf::from("c:/Users/me/My Repo"))
        );
        assert_eq!(file_uri_path("https://example.com/repo"), None);
        assert_eq!(file_uri_path("file:///"), None);
    }

    #[test]
    fn sqlite_timestamps_convert_with_offsets_but_not_the_zero_date() {
        assert_eq!(
            parse_time("2026-11-18 09:32:12.5207842+00:00"),
            Some(1_794_994_332)
        );
        assert_eq!(parse_time("2026-11-18 17:32:12+08:00"), Some(1_794_994_332));
        assert_eq!(parse_time("2026-11-18 09:32:12Z"), Some(1_794_994_332));
        assert_eq!(parse_time("0001-01-01 00:00:00+00:00"), None);
        assert_eq!(parse_time("not a timestamp"), None);
    }

    #[test]
    fn launch_replay_drops_print_and_conversation_selectors() {
        let argv = [
            "agy",
            "--model",
            "gemini",
            "-p",
            "hello",
            "--conversation",
            "old",
            "--sandbox",
            "--continue",
            "--add-dir",
            "/extra",
            "-i",
            "start here",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(
            launch_arguments(&argv).unwrap(),
            vec![
                "agy".to_string(),
                "--model".to_string(),
                "gemini".to_string(),
                "--sandbox".to_string(),
                "--add-dir".to_string(),
                "/extra".to_string(),
            ]
        );
        assert_eq!(launch_arguments(&[]).unwrap(), vec!["agy".to_string()]);
    }

    #[test]
    fn unknown_launch_options_fail_closed() {
        let argv = ["agy", "--mystery", "value"].map(str::to_string).to_vec();
        let error = launch_arguments(&argv).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Unsupported);
        assert!(error.message.contains("--mystery"));
    }
}
