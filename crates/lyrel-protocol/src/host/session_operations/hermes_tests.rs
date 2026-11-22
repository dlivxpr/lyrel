use super::*;
use base64::Engine as _;

#[test]
fn hermes_safe_mode_does_not_start_a_fake_fork() {
    let dir = tempfile::tempdir().unwrap();
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Fork,
        source_id: "20261018_193219_a2659f".into(),
        cwd: dir.path().into(),
        launch_argv: vec!["hermes".into(), "chat".into(), "--safe-mode".into()],
        scope: SessionScope::default(),
    };
    let error = crate::host::local::LocalHost::new()
        .session_operation(&request)
        .unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Unsupported);
    assert!(error.retained.is_none());
}

#[test]
fn hermes_profile_mismatch_cannot_fork_in_another_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut scope = SessionScope {
        profile: Some("work".into()),
        ..Default::default()
    };
    scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Fork,
        source_id: "20261018_193219_a2659f".into(),
        cwd: dir.path().into(),
        launch_argv: vec!["hermes".into(), "chat".into()],
        scope,
    };
    let error = crate::host::local::LocalHost::new()
        .session_operation(&request)
        .unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Identity);
    assert!(error.retained.is_none());
}

/// Exercises the selected installation without touching the user's Hermes home.
#[test]
#[ignore = "requires LYREL_TEST_HERMES pointing to an installed Hermes launcher"]
fn native_hermes_history_uses_selected_launcher_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let mut scope = SessionScope::default();
    scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: dir.path().into(),
        launch_argv: vec![std::env::var("LYREL_TEST_HERMES").unwrap()],
        scope,
    };
    let sessions = crate::host::local::LocalHost::new()
        .hermes_sessions(&request)
        .expect("the selected Hermes installation must expose native history");
    assert!(
        sessions.is_empty(),
        "an isolated home has no stored sessions"
    );
}

/// Reopen an existing recovery target without ever creating another fork.
#[test]
#[ignore = "requires LYREL_TEST_HERMES, LYREL_TEST_HERMES_HOME, LYREL_TEST_HERMES_SESSION and LYREL_TEST_HERMES_CWD"]
fn native_hermes_resume_releases_gateway_and_preserves_stored_session() {
    let host = crate::host::local::LocalHost::new();
    let mut request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: std::env::var("LYREL_TEST_HERMES_SESSION").unwrap(),
        cwd: PathBuf::from(std::env::var("LYREL_TEST_HERMES_CWD").unwrap()),
        launch_argv: vec![
            std::env::var("LYREL_TEST_HERMES").unwrap(),
            "chat".into(),
            "--cli".into(),
        ],
        scope: SessionScope::default(),
    };
    request.scope.environment.insert(
        "HERMES_HOME".into(),
        std::env::var("LYREL_TEST_HERMES_HOME").unwrap(),
    );
    let before = host.hermes_sessions(&request).unwrap();
    assert!(before.iter().any(|session| session.id == request.source_id));
    for _ in 0..2 {
        let resumed = host.session_operation(&request).unwrap();
        assert_eq!(
            resumed.target_id.as_deref(),
            Some(request.source_id.as_str())
        );
        assert!(!resumed.created);
        assert!(
            resumed
                .command
                .as_ref()
                .unwrap()
                .contains(&request.source_id)
        );
        assert_eq!(
            resumed.scope.environment.get("HERMES_HOME"),
            request.scope.environment.get("HERMES_HOME")
        );
    }
    let after = host.hermes_sessions(&request).unwrap();
    assert!(after.iter().any(|session| session.id == request.source_id));
    let mut before_ids = before.iter().map(|session| &session.id).collect::<Vec<_>>();
    let mut after_ids = after.iter().map(|session| &session.id).collect::<Vec<_>>();
    before_ids.sort();
    after_ids.sort();
    assert_eq!(
        before_ids, after_ids,
        "Resume must not create a recovery fork"
    );
}

/// Real official source-install launcher + native gateway, explicitly isolated by the caller.
#[test]
#[ignore = "requires LYREL_TEST_HERMES, LYREL_TEST_HERMES_HOME, LYREL_TEST_HERMES_SESSION and LYREL_TEST_HERMES_CWD"]
fn native_hermes_fork_retains_a_stored_identity_and_independent_source() {
    let host = crate::host::local::LocalHost::new();
    let mut request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Fork,
        source_id: std::env::var("LYREL_TEST_HERMES_SESSION").unwrap(),
        cwd: PathBuf::from(std::env::var("LYREL_TEST_HERMES_CWD").unwrap()),
        launch_argv: vec![
            std::env::var("LYREL_TEST_HERMES").unwrap(),
            "chat".into(),
            "--cli".into(),
        ],
        scope: SessionScope::default(),
    };
    request.scope.environment.insert(
        "HERMES_HOME".into(),
        std::env::var("LYREL_TEST_HERMES_HOME").unwrap(),
    );
    let history = host.hermes_sessions(&request).unwrap();
    let source = history
        .iter()
        .find(|session| session.id == request.source_id)
        .expect("native source is visible in the same home");
    assert_eq!(source.agent, CLIAgent::Hermes);
    assert_eq!(source.cwd.as_deref(), Some(request.cwd.as_path()));
    assert_eq!(
        source
            .scope
            .as_ref()
            .unwrap()
            .environment
            .get("HERMES_HOME"),
        request.scope.environment.get("HERMES_HOME")
    );
    let fork = host.session_operation(&request).unwrap();
    let target = fork.target_id.clone().unwrap();
    assert_ne!(target, request.source_id);
    assert!(fork.created);
    assert!(fork.command.as_ref().unwrap().contains(&target));
    let retained = fork
        .opening_failed("pane connection failed")
        .retained
        .unwrap();
    assert_eq!(retained.target_id.as_deref(), Some(target.as_str()));
    assert!(retained.created);
    request.kind = SessionOperationKind::Resume;
    let original = host.session_operation(&request).unwrap();
    assert_eq!(
        original.target_id.as_deref(),
        Some(request.source_id.as_str())
    );
    request.source_id = target.clone();
    let reopened = host.session_operation(&request).unwrap();
    assert_eq!(reopened.target_id.as_deref(), Some(target.as_str()));
    assert!(!reopened.created);
    let history = host.hermes_sessions(&request).unwrap();
    assert!(history.iter().any(|session| session.id == target));
    let mut other = request.clone();
    other.expected_host = 999;
    assert_eq!(
        host.hermes_sessions(&other).unwrap_err().kind,
        SessionErrorKind::WrongHost
    );
    request.source_id = "lyrel-missing-session-must-not-create".into();
    for kind in [SessionOperationKind::Resume, SessionOperationKind::Fork] {
        request.kind = kind;
        let error = host.session_operation(&request).unwrap_err();
        assert_eq!(error.kind, SessionErrorKind::Command);
        assert!(error.retained.is_none());
    }
}

#[test]
fn hermes_statistics_rejects_wrong_host_and_profile_without_starting_runtime() {
    let host = crate::host::local::LocalHost::new();
    let dir = tempfile::tempdir().unwrap();
    let mut request = SessionOperation {
        expected_host: 999,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: dir.path().into(),
        launch_argv: vec!["hermes".into()],
        scope: SessionScope::default(),
    };
    assert_eq!(
        host.hermes_statistics(&request).unwrap_err().kind,
        SessionErrorKind::WrongHost
    );
    request.expected_host = 0;
    request.scope.profile = Some("work".into());
    request.scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    assert_eq!(
        host.hermes_statistics(&request).unwrap_err().kind,
        SessionErrorKind::Identity
    );
}

#[cfg(windows)]
#[test]
fn hermes_history_preserves_result_while_owned_gateway_finishes_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let gateway = dir.path().join("gateway.ps1");
    std::fs::write(
        &gateway,
        r#"$ErrorActionPreference = 'Stop'
[void][Console]::ReadLine()
[Console]::WriteLine('{"jsonrpc":"2.0","id":"lyrel-hermes-operation","result":{"projects":[{"previewSessions":[{"id":"cleanup-session","source":"cli","title":"Stored history","last_active":1}]}]}}')
[Console]::Out.Flush()
[void][Console]::In.ReadToEnd()
Start-Sleep -Seconds 6
[IO.File]::WriteAllText((Join-Path $env:HERMES_HOME 'gateway-cleanup'), 'flushed')
"#,
    )
    .unwrap();
    let launcher = dir.path().join("hermes.cmd");
    // Build the runtime JSON in PowerShell, so Unicode home paths do not pass through cmd's code page.
    std::fs::write(
        &launcher,
        "@echo off\r\n\"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoProfile -NonInteractive -Command \"[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false); [Console]::WriteLine((ConvertTo-Json -InputObject @((Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'), '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $env:HERMES_HOME 'gateway.ps1')) -Compress))\"\r\n",
    )
    .unwrap();
    let mut scope = SessionScope::default();
    scope.environment.insert(
        "HERMES_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let request = SessionOperation {
        expected_host: 0,
        agent: CLIAgent::Hermes,
        kind: SessionOperationKind::Resume,
        source_id: String::new(),
        cwd: dir.path().into(),
        launch_argv: vec![launcher.to_string_lossy().into_owned()],
        scope,
    };
    let sessions = crate::host::local::LocalHost::new()
        .hermes_sessions(&request)
        .expect("valid stored history must survive cleanup within the operation deadline");
    assert_eq!(sessions[0].id, "cleanup-session");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("gateway-cleanup")).unwrap(),
        "flushed",
        "the owned gateway must finish its cleanup instead of being killed after its valid response"
    );
}

// ---- Bounded batch deletion (#367) ----

fn runtime_argv(entry: &str) -> Vec<String> {
    vec![
        "/opt/hermes/runtime/bin/python3".into(),
        "-I".into(),
        "-c".into(),
        format!("import runpy, sys; sys.path.insert(0, '/opt/hermes/pkg'); {entry}"),
    ]
}

#[test]
fn batch_adapter_substitutes_only_the_published_gateway_entry() {
    let argv = adapter_argv(&runtime_argv(GATEWAY_ENTRY_SUFFIX)).unwrap();
    assert_eq!(argv[0], "/opt/hermes/runtime/bin/python3");
    assert_eq!(argv[1], "-I");
    assert_eq!(argv[2], "-c");
    assert!(argv[3].starts_with("import runpy, sys; sys.path.insert(0, '/opt/hermes/pkg'); "));
    assert!(argv[3].contains("base64.b64decode"));
    assert!(
        !argv[3].contains(GATEWAY_ENTRY_SUFFIX),
        "the gateway module entry is gone"
    );
    let encoded = argv[3]
        .split("base64.b64decode(\"")
        .nth(1)
        .and_then(|rest| rest.split('\"').next())
        .expect("the adapter payload is embedded");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("the payload is valid base64");
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        BATCH_ADAPTER_SCRIPT,
        "the fixed adapter script travels byte-identical"
    );
}

#[test]
fn batch_adapter_refuses_any_other_runtime_shape() {
    for bad in [
        vec![
            "python3".into(),
            "-I".into(),
            "-c".into(),
            "print(1)".into(),
        ],
        vec![
            "python3".into(),
            "-I".into(),
            "-m".into(),
            "tui_gateway.entry".into(),
        ],
        runtime_argv("runpy.run_module('other.entry', run_name='__main__', alter_sys=True)"),
        vec!["python3".into(), "-I".into()],
    ] {
        let error = adapter_argv(&bad).unwrap_err();
        assert_eq!(
            error.kind,
            SessionErrorKind::Unsupported,
            "unrecognized shape refuses closed: {bad:?}"
        );
    }
}

#[test]
fn batch_plan_mapping_binds_each_answer_by_source_id_in_request_order() {
    let answers = serde_json::json!([
        {"id": "b", "status": "eligible", "delete_ids": ["b"]},
        {"id": "a", "status": "stale", "message": "gone"},
        {"id": "c", "status": "failed", "message": "native blew up"},
    ]);
    let result = plan_result(
        &[
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ],
        answers.as_array().unwrap(),
    );
    let items = &result.items;
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].source_id, "a");
    assert_eq!(items[0].status, batch::SessionBatchStatus::Ineligible);
    assert_eq!(
        items[0].reason.as_ref().unwrap().kind,
        batch::SessionBatchReasonKind::Stale
    );
    assert_eq!(items[1].source_id, "b");
    assert_eq!(items[1].status, batch::SessionBatchStatus::Eligible);
    assert_eq!(items[1].delete_ids, ["b"]);
    assert_eq!(items[2].source_id, "c");
    assert_eq!(
        items[2].reason.as_ref().unwrap().kind,
        batch::SessionBatchReasonKind::Provider
    );
    assert_eq!(
        items[3].reason.as_ref().unwrap().message,
        "adapter returned no answer for this session"
    );
}

#[test]
fn batch_execute_mapping_turns_guards_and_drift_into_skipped_rows() {
    let confirmations = vec![
        batch::SessionBatchConfirmation {
            source_id: "a".into(),
            expected_delete_ids: vec!["a".into(), "a.child".into()],
        },
        batch::SessionBatchConfirmation {
            source_id: "b".into(),
            expected_delete_ids: vec!["b".into()],
        },
        batch::SessionBatchConfirmation {
            source_id: "c".into(),
            expected_delete_ids: vec!["c".into()],
        },
        batch::SessionBatchConfirmation {
            source_id: "d".into(),
            expected_delete_ids: vec!["d".into()],
        },
    ];
    let answers = serde_json::json!([
        {"id": "d", "status": "deleted", "delete_ids": ["d"]},
        {"id": "c", "status": "deleted", "delete_ids": ["c"]},
        {"id": "b", "status": "guarded", "message": "active lease"},
        {"id": "a", "status": "stale", "message": "cascade drifted"},
    ]);
    let result = execute_result(&confirmations, answers.as_array().unwrap());
    let items = &result.items;
    assert_eq!(items[0].source_id, "a");
    assert_eq!(items[0].status, batch::SessionBatchStatus::Skipped);
    assert_eq!(
        items[0].reason.as_ref().unwrap().kind,
        batch::SessionBatchReasonKind::Stale
    );
    assert_eq!(items[1].source_id, "b");
    assert_eq!(
        items[1].reason.as_ref().unwrap().kind,
        batch::SessionBatchReasonKind::Active
    );
    assert_eq!(items[2].source_id, "c");
    assert_eq!(items[2].status, batch::SessionBatchStatus::Deleted);
    assert_eq!(items[3].source_id, "d");
    assert!(items[3].reason.is_none());
    // The reported delete scope is exactly the confirmed set, never broader.
    assert_eq!(items[2].delete_ids, ["c"]);
}

#[test]
#[ignore = "requires a disposable managed Hermes installation and LYREL_TEST_HERMES"]
fn native_hermes_batch_deletes_only_confirmed_delegate_scope() {
    let launcher = PathBuf::from(std::env::var_os("LYREL_TEST_HERMES").unwrap());
    assert!(launcher.is_absolute());
    let installation = launcher.parent().unwrap().parent().unwrap();
    let canonical = installation.canonicalize().unwrap();
    assert!(
        canonical
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("lyrel-363-hermes-")
    );
    assert!(canonical.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(launcher.canonicalize().unwrap().starts_with(&canonical));
    let storage = installation.join("home");
    assert_eq!(storage.canonicalize().unwrap(), canonical.join("home"));
    // Windows canonicalize adds a verbatim prefix for validation. Keep the
    // launcher's original spelling for Hermes, which compares PM paths.
    let cwd = tempfile::tempdir().unwrap();
    let mut scope = SessionScope::default();
    scope
        .environment
        .insert("HERMES_HOME".into(), storage.to_string_lossy().into_owned());
    scope.environment.insert(
        "HERMES_RUNTIME_DIR".into(),
        installation.join("runtime").to_string_lossy().into_owned(),
    );
    let id = format!("lyrel-fixture-{}", uuid::Uuid::new_v4());
    let child = format!("{id}-delegate");
    let branch = format!("{id}-branch");
    let prepare = batch::SessionBatchPrepareGroup {
        cwd: cwd.path().into(),
        launch_argv: vec![
            launcher.to_string_lossy().into_owned(),
            "chat".into(),
            "--cli".into(),
        ],
        scope,
        source_ids: vec![id.clone()],
    };
    let (gateway, scope, cwd) = prepare.gateway_scope_cwd().unwrap();
    let runtime = runtime_command(&gateway, &scope, &cwd).unwrap();
    let prefix = runtime[3].strip_suffix(GATEWAY_ENTRY_SUFFIX).unwrap();
    let script = r#"
import json, os, sys
from pathlib import Path
from hermes_state import SessionDB
from hermes_constants import get_hermes_home
request = json.load(sys.stdin)
root, child, branch = request['ids']
store = Path(os.environ['HERMES_HOME']).resolve(strict=True)
assert get_hermes_home().resolve() == store
with SessionDB(db_path=store / 'state.db') as db:
    action = request['action']
    if action == 'seed':
        db.create_session(root, 'cli', cwd=os.getcwd())
        db.create_session(child, 'subagent', cwd=os.getcwd(), parent_session_id=root, model_config={'_delegate_from': root})
        db.create_session(branch, 'cli', cwd=os.getcwd(), parent_session_id=root, model_config={'_branched_from': root})
        (store / 'sessions').mkdir(exist_ok=True)
        for session in (root, child, branch):
            (store / 'sessions' / (session + '.jsonl')).write_text('disposable session\n')
    if action == 'lock':
        assert db.try_acquire_session_turn_lease(child, root, ttl_seconds=180)
    elif action == 'unlock':
        db.release_session_turn_lease(child, root)
    print(json.dumps([db.get_session(s) is not None for s in (root, child, branch)]))
"#;
    let mut fixture_argv = runtime.clone();
    fixture_argv[3] = format!("{prefix}exec({})", serde_json::to_string(script).unwrap());
    let inspect = |action| {
        let payload = serde_json::json!({"action": action, "ids": [&id, &child, &branch]});
        serde_json::from_str::<[bool; 3]>(
            &adapter_line(&fixture_argv, &scope, &cwd, &payload).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(inspect("seed"), [true, true, true]);
    let host = crate::host::local::LocalHost::new();
    let plan = || {
        let reply = host
            .session_batch(&batch::SessionBatchRequest::Prepare(Box::new(
                batch::SessionBatchPrepare {
                    expected_host: 0,
                    agent: CLIAgent::Hermes,
                    groups: vec![prepare.clone()],
                },
            )))
            .unwrap();
        reply.group_results()[0].items[0].clone()
    };
    assert_eq!(inspect("lock"), [true, true, true]);
    let guarded = plan();
    assert_eq!(guarded.status, batch::SessionBatchStatus::Ineligible);
    assert_eq!(
        guarded.reason.unwrap().kind,
        batch::SessionBatchReasonKind::Active
    );
    assert_eq!(inspect("unlock"), [true, true, true]);
    let plan = plan();
    assert_eq!(plan.status, batch::SessionBatchStatus::Eligible, "{plan:?}");
    assert_eq!(plan.delete_ids, [id.clone(), child.clone()]);
    let execute = |expected_delete_ids| {
        host.session_batch(&batch::SessionBatchRequest::Execute(Box::new(
            batch::SessionBatchExecute {
                expected_host: 0,
                agent: CLIAgent::Hermes,
                groups: vec![batch::SessionBatchExecuteGroup {
                    cwd: prepare.cwd.clone(),
                    launch_argv: prepare.launch_argv.clone(),
                    scope: prepare.scope.clone(),
                    confirmations: vec![batch::SessionBatchConfirmation {
                        source_id: id.clone(),
                        expected_delete_ids,
                    }],
                }],
            },
        )))
        .unwrap()
    };
    let stale = execute(vec![id.clone()]);
    assert_eq!(
        stale.group_results()[0].items[0].status,
        batch::SessionBatchStatus::Skipped
    );
    assert_eq!(inspect("lock"), [true, true, true]);
    let drifted = execute(plan.delete_ids.clone());
    let item = &drifted.group_results()[0].items[0];
    assert_eq!(item.status, batch::SessionBatchStatus::Skipped);
    assert_eq!(
        item.reason.as_ref().unwrap().kind,
        batch::SessionBatchReasonKind::Active
    );
    assert_eq!(inspect("unlock"), [true, true, true]);
    let removed = execute(plan.delete_ids.clone());
    let item = &removed.group_results()[0].items[0];
    assert_eq!(item.status, batch::SessionBatchStatus::Deleted, "{item:?}");
    assert_eq!(inspect("read"), [false, false, true]);
    assert!(
        !storage
            .join("sessions")
            .join(format!("{id}.jsonl"))
            .exists()
    );
    assert!(
        !storage
            .join("sessions")
            .join(format!("{child}.jsonl"))
            .exists()
    );
    assert!(
        storage
            .join("sessions")
            .join(format!("{branch}.jsonl"))
            .is_file()
    );
}
