//! Hermes 0.22.2 native stored-session operations via the selected installation's runtime.
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

/// Statistics uses the public stored rows, never live session.usage/runtime IDs.
pub(crate) fn statistics(
    request: &SessionOperation,
) -> Result<serde_json::Value, SessionOperationError> {
    if request.agent != CLIAgent::Hermes {
        return Err(failure(
            SessionErrorKind::Identity,
            "Hermes statistics requires a Hermes launch context",
        ));
    }
    let (_, gateway, scope) = context(request)?;
    let (mut result, shutdown_error) = rpc(
        &gateway,
        &scope,
        &request.cwd,
        "projects.tree",
        serde_json::json!({"preview_limit":501,"session_limit":501}),
    )?;
    if let Some(error) = shutdown_error {
        return Err(failure(SessionErrorKind::Command, error));
    }
    if !result["projects"].is_array() || !result["scoped_session_ids"].is_array() {
        return Err(failure(
            SessionErrorKind::Command,
            "Hermes statistics returned invalid project metadata",
        ));
    }
    for project in result["projects"].as_array_mut().unwrap() {
        if let Some(rows) = project
            .get_mut("previewSessions")
            .and_then(|rows| rows.as_array_mut())
        {
            for row in rows {
                if let Some(fields) = row.as_object_mut() {
                    fields.remove("preview");
                    fields.remove("title");
                }
            }
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Bounded batch deletion (#367). The stock `session.delete` RPC forwards only
// the write-guard switch and `sessions export --delete-after-verified`
// computes its own later set; neither fences the exact confirmed cascade.
// The verified path below reuses the selected installation's own Python
// through the recognized `--print-runtime-command --module tui_gateway.entry`
// argv shape, substitutes only the `-c` entry with a fixed adapter script,
// and drives the native SessionDB with `expected_delete_ids` +
// `exclude_active_write_guards` inside one write transaction per target.
// ---------------------------------------------------------------------------

/// The exact suffix the official launcher publishes for the gateway entry;
/// verified on Windows and Linux against the pinned launcher fixture. Only
/// this shape may run the delete adapter — anything else refuses closed.
const GATEWAY_ENTRY_SUFFIX: &str =
    "runpy.run_module('tui_gateway.entry', run_name='__main__', alter_sys=True)";

/// The fixed adapter program. Inputs arrive as one structured JSON stdin
/// line (never interpolated); the store and the writer-lease fence both
/// resolve through this installation-bound runtime, not through a client-side
/// recheck. Each root is isolated: one bad target cannot hide the others.
const BATCH_ADAPTER_SCRIPT: &str = r#"
import inspect
import json
import os
import sys
from pathlib import Path


def emit(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def refuse(message):
    emit({"ok": False, "error": str(message)[:4000]})
    sys.exit(0)


def main():
    try:
        request = json.loads(sys.stdin.readline(64 * 1024))
    except Exception as error:
        refuse("invalid request: %s" % error)
        return
    try:
        home = Path(os.environ["HERMES_HOME"]).resolve(strict=True)
    except Exception as error:
        refuse("HERMES_HOME is not resolvable: %s" % error)
        return
    asked = request.get("home")
    if asked and Path(str(asked)).resolve() != home:
        refuse("requested home does not match the pinned HERMES_HOME")
        return
    try:
        from hermes_constants import get_hermes_home
        from hermes_state import SessionDB
        from hermes_state_errors import SessionActiveWriteGuardError
    except Exception as error:
        refuse("Hermes native state modules unavailable: %s" % error)
        return
    try:
        if Path(get_hermes_home()).resolve() != home:
            refuse("native home resolution disagrees with HERMES_HOME")
            return
    except Exception as error:
        refuse("native home resolution failed: %s" % error)
        return
    if not callable(getattr(SessionDB, "get_session_delete_targets", None)):
        refuse("installed Hermes lacks SessionDB.get_session_delete_targets")
        return
    signature = inspect.signature(SessionDB.delete_session).parameters
    for name in ("expected_delete_ids", "exclude_active_write_guards"):
        if name not in signature:
            refuse("installed Hermes delete_session lacks %s" % name)
            return
    mode = request.get("mode")
    roots = request.get("roots") or []
    expected = request.get("expected") or {}
    results = []
    db_path = home / "state.db"
    if not db_path.is_file() or db_path.resolve() != db_path:
        refuse("Hermes state.db is missing or points outside the confirmed store")
    with SessionDB(db_path=db_path) as db:
        sessions_dir = home / "sessions"
        if sessions_dir.exists() and sessions_dir.resolve() != sessions_dir:
            refuse("Hermes sessions directory points outside the confirmed store")
        for root in roots:
            try:
                if mode == "plan":
                    results.append(plan_one(db, root))
                elif mode == "delete":
                    results.append(delete_one(db, sessions_dir, root, expected.get(root)))
                else:
                    refuse("unknown adapter mode: %s" % mode)
                    return
            except Exception as error:
                results.append({"id": root, "status": "failed", "message": str(error)[:400]})
    emit({"ok": True, "results": results})


def plan_one(db, root):
    targets = db.get_session_delete_targets(root)
    if not targets:
        return {"id": root, "status": "stale", "message": "stored session was not found"}
    row = db.get_session(root) or {}
    if str(row.get("source") or "") != "cli":
        return {"id": root, "status": "stale", "message": "stored session is no longer a CLI session"}
    if row.get("archived"):
        return {"id": root, "status": "stale", "message": "stored session is archived"}
    return {"id": root, "status": "eligible", "delete_ids": list(targets)}


def delete_one(db, sessions_dir, root, expected):
    if expected is None:
        return {"id": root, "status": "failed", "message": "no confirmed delete scope"}
    plan = plan_one(db, root)
    if plan["status"] != "eligible":
        return plan
    if set(plan["delete_ids"]) != set(expected):
        return {"id": root, "status": "stale", "message": "native cascade changed since confirmation"}
    try:
        removed = db.delete_session(
            root,
            sessions_dir=sessions_dir,
            expected_delete_ids=[str(item) for item in expected],
            exclude_active_write_guards=True,
        )
    except SessionActiveWriteGuardError as error:
        return {"id": root, "status": "guarded", "message": str(error)[:400]}
    if not removed:
        return {"id": root, "status": "stale", "message": "native cascade changed since confirmation"}
    return {"id": root, "status": "deleted", "delete_ids": [str(item) for item in expected]}


main()
"#;

/// Run a bounded batch in request order. A failed store contributes its own
/// outcomes without hiding results from stores already processed.
pub(crate) fn batch(
    request: &batch::SessionBatchRequest,
) -> Result<batch::SessionBatchReply, SessionOperationError> {
    match request {
        batch::SessionBatchRequest::Prepare(request) => {
            let mut result = batch::SessionBatchPrepareResult::default();
            for group in &request.groups {
                let payload = serde_json::json!({
                    "home": group.scope.environment.get("HERMES_HOME"),
                    "mode": "plan",
                    "roots": group.source_ids,
                });
                let group_result = match batch_group(group, payload) {
                    Ok(answers) => plan_result(&group.source_ids, &answers),
                    Err(error) => failed_group(
                        group.source_ids.iter().map(String::as_str),
                        batch::SessionBatchStatus::Ineligible,
                        error,
                    ),
                };
                result.groups.push(group_result);
            }
            Ok(batch::SessionBatchReply::Prepare(result))
        }
        batch::SessionBatchRequest::Execute(request) => {
            let mut result = batch::SessionBatchExecuteResult::default();
            for group in &request.groups {
                let expected: serde_json::Map<String, serde_json::Value> = group
                    .confirmations
                    .iter()
                    .map(|confirmation| {
                        (
                            confirmation.source_id.clone(),
                            serde_json::json!(confirmation.expected_delete_ids),
                        )
                    })
                    .collect();
                let roots: Vec<&str> = group
                    .confirmations
                    .iter()
                    .map(|confirmation| confirmation.source_id.as_str())
                    .collect();
                let payload = serde_json::json!({
                    "home": group.scope.environment.get("HERMES_HOME"),
                    "mode": "delete",
                    "roots": roots,
                    "expected": expected,
                });
                let group_result = match batch_group(group, payload) {
                    Ok(answers) => execute_result(&group.confirmations, &answers),
                    Err(error) => {
                        failed_group(roots.into_iter(), batch::SessionBatchStatus::Failed, error)
                    }
                };
                result.groups.push(group_result);
            }
            Ok(batch::SessionBatchReply::Execute(result))
        }
    }
}

fn failed_group<'a>(
    ids: impl Iterator<Item = &'a str>,
    status: batch::SessionBatchStatus,
    error: SessionOperationError,
) -> batch::SessionBatchGroupResult {
    let kind = match error.kind {
        SessionErrorKind::Unsupported => batch::SessionBatchReasonKind::UnsupportedProvider,
        SessionErrorKind::Directory | SessionErrorKind::Identity => {
            batch::SessionBatchReasonKind::UnconfirmedStore
        }
        _ => batch::SessionBatchReasonKind::Provider,
    };
    let items = ids
        .map(|id| {
            batch::SessionBatchItem::refused(
                id.to_owned(),
                status,
                batch::SessionBatchReason::new(kind, error.message.clone()),
            )
        })
        .collect();
    batch::SessionBatchGroupResult { items }
}

/// Resolve the group's launch context through the same rules as history and
/// resume (one effective home/profile), then run the fixed adapter through
/// the installation's own runtime command and parse its one-line answer.
fn batch_group(
    group: &impl BatchGroupContext,
    payload: serde_json::Value,
) -> Result<Vec<serde_json::Value>, SessionOperationError> {
    let (gateway, scope, cwd) = group.gateway_scope_cwd()?;
    let runtime = runtime_command(&gateway, &scope, &cwd)?;
    let argv = adapter_argv(&runtime)?;
    let home = scope
        .environment
        .get("HERMES_HOME")
        .cloned()
        .ok_or_else(|| {
            failure(
                SessionErrorKind::Directory,
                "Hermes batch scope has no HERMES_HOME",
            )
        })?;
    let mut payload = payload;
    payload["home"] = serde_json::Value::String(home);
    let line = adapter_line(&argv, &scope, &cwd, &payload)?;
    let answer: serde_json::Value = serde_json::from_str(&line).map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "Hermes batch adapter returned no valid JSON",
        )
    })?;
    if answer["ok"] != true {
        return Err(failure(
            SessionErrorKind::Command,
            format!(
                "Hermes batch adapter: {}",
                answer["error"].as_str().unwrap_or("no adapter answer")
            ),
        ));
    }
    answer["results"].as_array().cloned().ok_or_else(|| {
        failure(
            SessionErrorKind::Command,
            "Hermes batch adapter returned no result list",
        )
    })
}

/// Both batch group shapes share the context resolution; the trait keeps the
/// generic dispatch out of the wire types.
trait BatchGroupContext {
    fn gateway_scope_cwd(
        &self,
    ) -> Result<(Vec<String>, SessionScope, PathBuf), SessionOperationError>;
}

impl BatchGroupContext for batch::SessionBatchPrepareGroup {
    fn gateway_scope_cwd(
        &self,
    ) -> Result<(Vec<String>, SessionScope, PathBuf), SessionOperationError> {
        let shim = SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Hermes,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: self.cwd.clone(),
            launch_argv: self.launch_argv.clone(),
            scope: self.scope.clone(),
        };
        let (_, gateway, scope) = context(&shim)?;
        Ok((gateway, scope, self.cwd.clone()))
    }
}

impl BatchGroupContext for batch::SessionBatchExecuteGroup {
    fn gateway_scope_cwd(
        &self,
    ) -> Result<(Vec<String>, SessionScope, PathBuf), SessionOperationError> {
        let shim = SessionOperation {
            expected_host: 0,
            agent: CLIAgent::Hermes,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: self.cwd.clone(),
            launch_argv: self.launch_argv.clone(),
            scope: self.scope.clone(),
        };
        let (_, gateway, scope) = context(&shim)?;
        Ok((gateway, scope, self.cwd.clone()))
    }
}

/// Substitute only the published gateway entry with the fixed adapter
/// program. The bootstrap prefix (interpreter setup, sys.path) is upstream's
/// own and stays byte-identical; the adapter source travels base64 so no
/// quoting or interpolation can corrupt it.
fn adapter_argv(runtime: &[String]) -> Result<Vec<String>, SessionOperationError> {
    if runtime.len() != 4
        || runtime[1] != "-I"
        || runtime[2] != "-c"
        || !runtime[3].ends_with(GATEWAY_ENTRY_SUFFIX)
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Hermes launcher runtime has an unrecognized gateway entry; only the published --print-runtime-command tui_gateway.entry shape can run the verified delete adapter",
        ));
    }
    let prefix = &runtime[3][..runtime[3].len() - GATEWAY_ENTRY_SUFFIX.len()];
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        BATCH_ADAPTER_SCRIPT.as_bytes(),
    );
    let program = format!(
        "{prefix}import base64;exec(compile(base64.b64decode(\"{encoded}\").decode(\"utf-8\"),\"lyrel-hermes-batch\",\"exec\"))"
    );
    Ok(vec![runtime[0].clone(), "-I".into(), "-c".into(), program])
}

/// Spawn the adapter runtime, feed it the one-line structured request on
/// stdin, and read its one-line answer. Response and teardown share one
/// absolute budget; a timeout never leaves the interpreter running.
fn adapter_line(
    argv: &[String],
    scope: &SessionScope,
    cwd: &Path,
    payload: &serde_json::Value,
) -> Result<String, SessionOperationError> {
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .envs(&scope.environment)
        .env_remove("LYREL")
        .env_remove(crate::daemon::LYREL_PANE_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::core::proc::hide_console(&mut command);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut child = command.spawn().map_err(|e| {
        failure(
            SessionErrorKind::Command,
            format!("Hermes batch adapter could not start: {e}"),
        )
    })?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if let Err(error) = writeln!(stdin, "{payload}").and_then(|_| stdin.flush()) {
        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure(
            SessionErrorKind::Command,
            format!("Hermes batch adapter request failed: {error}"),
        ));
    }
    drop(stdin);
    // The answer is one line; the reader threads keep the wait loop free to
    // enforce the deadline even when the adapter never speaks.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        let mut frame = Vec::new();
        let _ = reader
            .by_ref()
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut frame);
        let _ = tx.send(frame);
        let _ = io::copy(&mut reader, &mut io::sink());
    });
    let (error_tx, error_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(64 * 1024).read_to_end(&mut bytes);
        let _ = error_tx.send(bytes);
    });
    let stderr_bytes = |error_rx: &std::sync::mpsc::Receiver<Vec<u8>>| {
        error_rx
            .recv_timeout(Duration::from_millis(100))
            .unwrap_or_default()
    };
    let frame = loop {
        match rx.try_recv() {
            Ok(frame) => break Some(frame),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                match child.try_wait() {
                    // EOF already closed the pipe, so the reader's send is
                    // in flight: a short blocking recv beats racing it.
                    Ok(Some(_)) => {
                        break match rx.recv_timeout(Duration::from_millis(500)) {
                            Ok(frame) => Some(frame),
                            Err(_) => Some(Vec::new()),
                        };
                    }
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break None,
        }
    };
    let Some(frame) = frame else {
        return Err(failure(
            SessionErrorKind::Command,
            format!(
                "Hermes batch adapter returned no answer in time: {}",
                String::from_utf8_lossy(&stderr_bytes(&error_rx))
                    .chars()
                    .take(1000)
                    .collect::<String>()
            ),
        ));
    };
    // The answer arrived; the process still has to exit inside the budget.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failure(
                    SessionErrorKind::Command,
                    "Hermes batch adapter did not exit inside the operation budget",
                ));
            }
        }
    };
    if !status.success() {
        return Err(failure(
            SessionErrorKind::Command,
            format!(
                "Hermes batch adapter exited unsuccessfully ({status}): {}",
                String::from_utf8_lossy(&stderr_bytes(&error_rx))
                    .chars()
                    .take(1000)
                    .collect::<String>()
            ),
        ));
    }
    if frame.is_empty() || frame.len() > 1024 * 1024 {
        return Err(failure(
            SessionErrorKind::Command,
            format!(
                "Hermes batch adapter returned no answer: {}",
                String::from_utf8_lossy(&stderr_bytes(&error_rx))
                    .chars()
                    .take(1000)
                    .collect::<String>()
            ),
        ));
    }
    String::from_utf8(frame).map_err(|_| {
        failure(
            SessionErrorKind::Command,
            "Hermes batch adapter answer was not UTF-8",
        )
    })
}

/// Map the adapter's plan answers onto the prepare reply, keeping the
/// request's source order (the adapter answers in root order, which is the
/// request's order — this re-binds by id so a drifted adapter cannot
/// mislabel a row).
fn plan_result(
    source_ids: &[String],
    answers: &[serde_json::Value],
) -> batch::SessionBatchGroupResult {
    let find = |id: &str| {
        answers
            .iter()
            .find(|answer| answer["id"].as_str() == Some(id))
    };
    let items = source_ids
        .iter()
        .map(|id| match find(id) {
            Some(answer) if answer["status"] == "eligible" => {
                let delete_ids = answer["delete_ids"]
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                batch::SessionBatchItem::eligible(id.clone(), delete_ids)
            }
            Some(answer) if answer["status"] == "stale" => batch::SessionBatchItem::refused(
                id.clone(),
                batch::SessionBatchStatus::Ineligible,
                batch::SessionBatchReason::new(
                    batch::SessionBatchReasonKind::Stale,
                    answer["message"]
                        .as_str()
                        .unwrap_or("stored session changed"),
                ),
            ),
            Some(answer) => batch::SessionBatchItem::refused(
                id.clone(),
                batch::SessionBatchStatus::Ineligible,
                batch::SessionBatchReason::new(
                    batch::SessionBatchReasonKind::Provider,
                    answer["message"].as_str().unwrap_or("native plan failed"),
                ),
            ),
            None => batch::SessionBatchItem::refused(
                id.clone(),
                batch::SessionBatchStatus::Ineligible,
                batch::SessionBatchReason::new(
                    batch::SessionBatchReasonKind::Provider,
                    "adapter returned no answer for this session",
                ),
            ),
        })
        .collect();
    batch::SessionBatchGroupResult { items }
}

/// Map the adapter's delete answers onto the execute reply, binding each by
/// its confirmed source id. `guarded` is an active writer lease — the row
/// stays intact and the reason says who held it.
fn execute_result(
    confirmations: &[batch::SessionBatchConfirmation],
    answers: &[serde_json::Value],
) -> batch::SessionBatchGroupResult {
    let find = |id: &str| {
        answers
            .iter()
            .find(|answer| answer["id"].as_str() == Some(id))
    };
    let items = confirmations
        .iter()
        .map(|confirmation| {
            let id = &confirmation.source_id;
            match find(id) {
                Some(answer) if answer["status"] == "deleted" => batch::SessionBatchItem {
                    source_id: id.clone(),
                    status: batch::SessionBatchStatus::Deleted,
                    delete_ids: confirmation.expected_delete_ids.clone(),
                    reason: None,
                },
                Some(answer) if answer["status"] == "guarded" => batch::SessionBatchItem::refused(
                    id.clone(),
                    batch::SessionBatchStatus::Skipped,
                    batch::SessionBatchReason::new(
                        batch::SessionBatchReasonKind::Active,
                        answer["message"]
                            .as_str()
                            .unwrap_or("a writer lease holds this session"),
                    ),
                ),
                Some(answer) if answer["status"] == "stale" => batch::SessionBatchItem::refused(
                    id.clone(),
                    batch::SessionBatchStatus::Skipped,
                    batch::SessionBatchReason::new(
                        batch::SessionBatchReasonKind::Stale,
                        answer["message"].as_str().unwrap_or("native scope changed"),
                    ),
                ),
                Some(answer) => batch::SessionBatchItem::refused(
                    id.clone(),
                    batch::SessionBatchStatus::Failed,
                    batch::SessionBatchReason::new(
                        batch::SessionBatchReasonKind::Provider,
                        answer["message"].as_str().unwrap_or("native delete failed"),
                    ),
                ),
                None => batch::SessionBatchItem::refused(
                    id.clone(),
                    batch::SessionBatchStatus::Failed,
                    batch::SessionBatchReason::new(
                        batch::SessionBatchReasonKind::Provider,
                        "adapter returned no answer for this session",
                    ),
                ),
            }
        })
        .collect();
    batch::SessionBatchGroupResult { items }
}

pub(super) fn prepare(
    request: &SessionOperation,
) -> Result<PreparedSession, SessionOperationError> {
    let (launch, gateway, scope) = context(request)?;
    // Validate shell representation before any irreversible creation.
    agent_command_line(&launch, &scope, CLIAgent::Hermes)?;
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
    let command = agent_command_line(&open, &scope, CLIAgent::Hermes);
    let prepared = PreparedSession {
        host_id: request.expected_host,
        agent: CLIAgent::Hermes,
        source_id: request.source_id.clone(),
        target_id: Some(target.into()),
        session_file: None,
        title: None,
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
        .any(|key| !HOME_ENV.contains(&key.as_str()) && !TERMINAL_SCOPE_ENV.contains(&key.as_str()))
    {
        return Err(failure(
            SessionErrorKind::Unsupported,
            "Hermes accepts only home/runtime and public terminal environment selectors",
        ));
    }
    let mut argv = request.launch_argv.as_slice();
    while let Some((key, value)) = argv.first().and_then(|arg| arg.split_once('=')) {
        if !HOME_ENV.contains(&key) && !TERMINAL_SCOPE_ENV.contains(&key) {
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

/// Packaged POSIX launchers (including Homebrew) publish their interpreter as
/// HERMES_PYTHON instead of implementing the source installer's query flags.
/// Accept only a single-line env/exec wrapper and a literal absolute executable;
/// never evaluate shell expressions or substitute an ambient system Python.
#[cfg(unix)]
fn packaged_runtime_command(launcher: &[String], cwd: &Path) -> Option<Vec<String>> {
    // Do not bypass an explicitly configured interpreter or wrapper with arguments.
    if launcher.len() != 4 {
        return None;
    }
    let program = Path::new(&launcher[0]);
    let executable = if program.components().count() > 1 {
        cwd.join(program)
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| cwd.join(dir).join(program))
            .find(|path| {
                crate::core::cli_agent::program_on_path(
                    &path.to_string_lossy(),
                    std::ffi::OsStr::new(""),
                )
            })?
    };
    let mut script = String::new();
    std::fs::File::open(executable)
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_string(&mut script)
        .ok()?;
    if script.len() > 64 * 1024 || !matches!(script.lines().next()?, "#!/bin/bash" | "#!/bin/sh") {
        return None;
    }
    let mut lines = script
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let (environment, _) = lines.next()?.split_once(" exec ")?;
    if lines.next().is_some() || environment.matches("HERMES_PYTHON=").count() != 1 {
        return None;
    }
    static PYTHON: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?:^|[ \t])HERMES_PYTHON=("[^"\r\n]*"|'[^'\r\n]*'|[^ \t\r\n]+)(?:[ \t]|$)"#,
        )
        .unwrap()
    });
    let value = PYTHON.captures(environment)?.get(1)?;
    let mut python = crate::core::cli_agent::parse_command_argv(value.as_str()).ok()?;
    if python.len() != 1
        || !Path::new(&python[0]).is_absolute()
        || !crate::core::cli_agent::program_on_path(&python[0], std::ffi::OsStr::new(""))
    {
        return None;
    }
    // Keep the declared venv path: canonicalizing its Python symlink loses the
    // package's site-packages. Isolated mode also excludes the workspace CWD.
    python.extend(["-I".into(), "-m".into(), "tui_gateway.entry".into()]);
    Some(python)
}

fn runtime_command(
    launcher: &[String],
    scope: &SessionScope,
    cwd: &Path,
) -> Result<Vec<String>, SessionOperationError> {
    #[cfg(unix)]
    if let Some(runtime) = packaged_runtime_command(launcher, cwd) {
        return Ok(runtime);
    }
    let mut command = Command::new(&launcher[0]);
    command
        .args(&launcher[1..])
        .current_dir(cwd)
        .envs(&scope.environment)
        .env_remove("LYREL")
        .env_remove(crate::daemon::LYREL_PANE_ENV);
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
                "The selected Hermes launcher provides neither a literal HERMES_PYTHON wrapper nor --print-runtime-command --module tui_gateway.entry ({}): {}",
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
    // Resolve the selected installation's Python/bootstrap argv directly, so
    // Windows .cmd wrappers cannot retain a gateway child on timeout.
    let runtime = runtime_command(gateway, scope, cwd)?;
    let mut command = Command::new(&runtime[0]);
    command
        .args(&runtime[1..])
        .current_dir(cwd)
        .envs(&scope.environment)
        .env_remove("LYREL")
        .env_remove(crate::daemon::LYREL_PANE_ENV)
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
    // Response, native close, and process teardown share one absolute operation budget.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < response_deadline => {
                std::thread::sleep(Duration::from_millis(25))
            }
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

#[cfg(test)]
#[path = "hermes_tests.rs"]
mod tests;
