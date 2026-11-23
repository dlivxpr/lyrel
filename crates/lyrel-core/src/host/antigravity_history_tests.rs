//! Real fixture coverage for #385: the Antigravity conversation index is a
//! real SQLite database in the observed 1.5.15 shape, read through the full
//! local Host's read-only engine. Nothing here invokes `agy`; the preparation
//! assertions stop at the documented `--conversation` command it would type.

use std::path::{Path, PathBuf};

use lyrel_protocol::core::cli_agent::CLIAgent;
use lyrel_protocol::host::Host;
use lyrel_protocol::host::session_operations::antigravity;
use lyrel_protocol::host::session_operations::{
    SessionErrorKind, SessionOperation, SessionOperationKind, SessionScope,
};

use rusqlite::{Connection, params};

const UP_TO_DATE: i64 = 3;
const ROOT_ID: &str = "51919b0d-0f7e-4f31-9d5c-81321e4d3f50";
const SECOND_ID: &str = "9f8a56b3-2f9a-4d7e-8b15-271ff8a12b1b";
const OTHER_ID: &str = "d60dc3a8-5cb7-4d55-8c8a-3c3f1a5f0c95";

fn home_environment(home: &Path) -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        (
            "USERPROFILE".to_string(),
            home.to_string_lossy().into_owned(),
        ),
    ])
}

fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

fn open_index(home: &Path, version: i64) -> Connection {
    let root = home.join(".gemini").join("antigravity-cli");
    std::fs::create_dir_all(root.join("conversations")).unwrap();
    let connection = Connection::open(root.join("conversation_summaries.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE search_schema_version (version INTEGER PRIMARY KEY);
             CREATE TABLE conversation_summaries (
                conversation_id TEXT,
                title TEXT NOT NULL DEFAULT '',
                preview TEXT NOT NULL DEFAULT '',
                step_count INTEGER NOT NULL DEFAULT 0,
                last_modified_time TEXT NOT NULL,
                workspace_uris TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL DEFAULT '',
                project_id TEXT NOT NULL DEFAULT '',
                agent_name TEXT NOT NULL DEFAULT '',
                parent_conversation_id TEXT NOT NULL DEFAULT '',
                nesting_depth INTEGER NOT NULL DEFAULT 0,
                battle_id TEXT NOT NULL DEFAULT '',
                winning_conversation_id TEXT NOT NULL DEFAULT '',
                not_fully_idle NUMERIC NOT NULL DEFAULT 0,
                killed NUMERIC NOT NULL DEFAULT 0,
                last_user_input_time TEXT,
                last_user_input_step_index INTEGER,
                app_data_dir TEXT NOT NULL DEFAULT '',
                raw_summary BLOB,
                group_id TEXT NOT NULL DEFAULT ''
             );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO search_schema_version (version) VALUES (?1)",
            [version],
        )
        .unwrap();
    connection
}

#[allow(clippy::too_many_arguments)]
fn insert(
    connection: &Connection,
    id: &str,
    title: &str,
    preview: &str,
    updated: &str,
    workspace: &Path,
    parent: &str,
    depth: i64,
    app_data_dir: &str,
) {
    connection
        .execute(
            "INSERT INTO conversation_summaries (
                conversation_id, title, preview, last_modified_time,
                workspace_uris, parent_conversation_id, nesting_depth, app_data_dir
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                title,
                preview,
                updated,
                format!("[\"{}\"]", file_uri(workspace)),
                parent,
                depth,
                app_data_dir,
            ],
        )
        .unwrap();
}

fn conversation_store(home: &Path, id: &str) {
    let root = home.join(".gemini").join("antigravity-cli");
    std::fs::write(
        root.join("conversations").join(format!("{id}.db")),
        b"fixture",
    )
    .unwrap();
    let transcript = root
        .join("brain")
        .join(id)
        .join(".system_generated")
        .join("logs");
    std::fs::create_dir_all(&transcript).unwrap();
    std::fs::write(transcript.join("transcript.jsonl"), b"{}\n").unwrap();
}

fn request(
    host: &dyn Host,
    home: &Path,
    cwd: &Path,
    id: &str,
    kind: SessionOperationKind,
) -> SessionOperation {
    SessionOperation {
        expected_host: host.id().0,
        agent: CLIAgent::Antigravity,
        kind,
        source_id: id.to_string(),
        cwd: cwd.to_path_buf(),
        launch_argv: vec!["agy".into()],
        scope: SessionScope {
            environment: home_environment(home),
            shell_program: Some(if cfg!(windows) { "pwsh" } else { "sh" }.into()),
            ..Default::default()
        },
    }
}

fn paths_equal(actual: Option<&Path>, expected: &Path) -> bool {
    actual.is_some_and(|actual| {
        actual.to_string_lossy().replace('\\', "/") == expected.to_string_lossy().replace('\\', "/")
    })
}

#[test]
fn history_reads_only_root_cli_conversations_and_scopes_the_directory() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let index = open_index(home.path(), UP_TO_DATE);
    insert(
        &index,
        ROOT_ID,
        "Fix the login redirect",
        "the redirect loops",
        "2026-11-18 09:32:12.5207842+00:00",
        project.path(),
        "",
        0,
        "antigravity-cli",
    );
    insert(
        &index,
        SECOND_ID,
        "",
        "second prompt",
        "2026-11-17 14:55:04.9240173+00:00",
        project.path(),
        "",
        0,
        "antigravity-cli",
    );
    insert(
        &index,
        OTHER_ID,
        "Other checkout",
        "",
        "2026-11-16 16:18:41.207516+00:00",
        other.path(),
        "",
        0,
        "antigravity-cli",
    );
    insert(
        &index,
        "desktop-app-conversation",
        "Desktop app",
        "",
        "2026-11-16 16:18:41.207516+00:00",
        project.path(),
        "",
        0,
        "antigravity",
    );
    insert(
        &index,
        "subagent-conversation",
        "Subagent",
        "",
        "2026-11-16 16:18:41.207516+00:00",
        project.path(),
        ROOT_ID,
        1,
        "antigravity-cli",
    );
    // A row with the observed zero timestamp has never been used; it is not
    // a resumable saved conversation.
    insert(
        &index,
        "zero-date-conversation",
        "",
        "",
        "0001-01-01 00:00:00+00:00",
        project.path(),
        "",
        0,
        "antigravity-cli",
    );
    drop(index);

    let host = crate::host::local::new();
    let request = request(
        &*host,
        home.path(),
        project.path(),
        "",
        SessionOperationKind::Resume,
    );
    let found = antigravity::sessions(&*host, Some(project.path()), &request).unwrap();
    assert_eq!(
        found
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        [ROOT_ID, SECOND_ID],
        "directory-scoped, root CLI conversations only"
    );
    assert_eq!(found[0].title, "Fix the login redirect");
    assert_eq!(found[1].title, "second prompt");
    assert_eq!(found[0].updated, 1_794_994_332);
    assert_eq!(found[0].agent, CLIAgent::Antigravity);
    assert!(
        paths_equal(found[0].cwd.as_deref(), project.path()),
        "the recorded workspace is decoded from its file URI"
    );
    let scope = found[0].scope.as_ref().expect("a native scope");
    let transcript = scope.session_file.as_deref().expect("exact transcript");
    assert!(
        transcript.ends_with(
            Path::new("brain")
                .join(ROOT_ID)
                .join(".system_generated/logs/transcript.jsonl")
        ) || transcript
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(&format!(
                "brain/{ROOT_ID}/.system_generated/logs/transcript.jsonl"
            )),
        "the scope names the exact native transcript: {}",
        transcript.display()
    );

    let all = antigravity::sessions(&*host, None, &request).unwrap();
    assert_eq!(
        all.iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        [ROOT_ID, SECOND_ID, OTHER_ID],
        "an unscoped listing keeps other directories but not another app store or a subagent"
    );

    let absent = antigravity::sessions(
        &*host,
        Some(Path::new("/definitely/not/the/recorded/directory")),
        &request,
    )
    .unwrap();
    assert!(
        absent.is_empty(),
        "a directory with no rows is a normal empty answer"
    );
}

#[test]
fn a_missing_or_unreadable_store_keeps_other_sources_usable() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let host = crate::host::local::new();
    let request = request(
        &*host,
        home.path(),
        project.path(),
        "",
        SessionOperationKind::Resume,
    );

    let absent = antigravity::sessions(&*host, None, &request).unwrap();
    assert!(
        absent.is_empty(),
        "Antigravity never used on this Host is an empty source, not an error"
    );

    let unknown = tempfile::tempdir().unwrap();
    let index = open_index(unknown.path(), UP_TO_DATE + 1);
    drop(index);
    let unknown_request = request(
        &*host,
        unknown.path(),
        project.path(),
        "",
        SessionOperationKind::Resume,
    );
    let error = antigravity::sessions(&*host, None, &unknown_request).unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Unsupported);
    assert!(
        error.message.contains("schema v4"),
        "the unknown index version is named, not guessed: {}",
        error.message
    );

    let bare = lyrel_protocol::host::local::LocalHost::new();
    let bare_request = request(
        &*bare,
        home.path(),
        project.path(),
        "",
        SessionOperationKind::Resume,
    );
    let error = antigravity::sessions(&*bare, None, &bare_request).unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Unsupported);
    assert!(
        error.message.contains("read-only") || error.message.contains("cannot browse"),
        "a transport without the read-only browser explains the capability gap: {}",
        error.message
    );
}

#[test]
fn exact_resume_revalidates_identity_directory_and_fork_boundary() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let index = open_index(home.path(), UP_TO_DATE);
    insert(
        &index,
        ROOT_ID,
        "Fix the login redirect",
        "",
        "2026-11-18 09:32:12.5207842+00:00",
        project.path(),
        "",
        0,
        "antigravity-cli",
    );
    drop(index);
    conversation_store(home.path(), ROOT_ID);

    let host = crate::host::local::new();
    let resume = request(
        &*host,
        home.path(),
        project.path(),
        ROOT_ID,
        SessionOperationKind::Resume,
    );
    let prepared = host.session_operation(&resume).unwrap();
    assert_eq!(prepared.agent, CLIAgent::Antigravity);
    assert_eq!(prepared.source_id, ROOT_ID);
    assert_eq!(prepared.target_id.as_deref(), Some(ROOT_ID));
    assert!(!prepared.created);
    assert_eq!(prepared.title.as_deref(), Some("Fix the login redirect"));
    let command = prepared.command.expect("an executable command");
    assert!(command.contains("--conversation"), "{command}");
    assert!(command.contains(ROOT_ID), "{command}");
    let transcript = prepared.session_file.expect("the exact transcript");
    assert!(
        transcript
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(&format!(
                "brain/{ROOT_ID}/.system_generated/logs/transcript.jsonl"
            )),
        "{}",
        transcript.display()
    );

    let wrong_directory = request(
        &*host,
        home.path(),
        elsewhere.path(),
        ROOT_ID,
        SessionOperationKind::Resume,
    );
    assert_eq!(
        host.session_operation(&wrong_directory).unwrap_err().kind,
        SessionErrorKind::Identity,
        "an exact resume must not open the conversation in another directory"
    );

    let wrong_identity = request(
        &*host,
        home.path(),
        project.path(),
        "00000000-0000-4000-8000-000000000000",
        SessionOperationKind::Resume,
    );
    assert_eq!(
        host.session_operation(&wrong_identity).unwrap_err().kind,
        SessionErrorKind::Identity,
        "a conversation id the index does not hold is refused"
    );

    let root = home.path().join(".gemini").join("antigravity-cli");
    let index_bytes = std::fs::read(root.join("conversation_summaries.db")).unwrap();
    let conversation_bytes =
        std::fs::read(root.join("conversations").join(format!("{ROOT_ID}.db"))).unwrap();
    let fork = request(
        &*host,
        home.path(),
        project.path(),
        ROOT_ID,
        SessionOperationKind::Fork,
    );
    let error = host.session_operation(&fork).unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Unsupported);
    assert!(
        error.message.contains("independent fork route") && error.message.contains("/fork"),
        "the gate names the limitation and the manual route: {}",
        error.message
    );
    assert_eq!(
        std::fs::read(root.join("conversation_summaries.db")).unwrap(),
        index_bytes,
        "a refused fork leaves the source index byte-identical"
    );
    assert_eq!(
        std::fs::read(root.join("conversations").join(format!("{ROOT_ID}.db"))).unwrap(),
        conversation_bytes,
        "a refused fork leaves the source conversation byte-identical"
    );

    // The gate answers before the store is consulted: an empty home still
    // reads as Unsupported, not as a missing conversation.
    let empty_home = tempfile::tempdir().unwrap();
    let empty_project = tempfile::tempdir().unwrap();
    let missing = request(
        &*host,
        empty_home.path(),
        empty_project.path(),
        "00000000-0000-4000-8000-000000000000",
        SessionOperationKind::Fork,
    );
    let error = host.session_operation(&missing).unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Unsupported);
    assert!(
        error.message.contains("independent fork route"),
        "the same gate answers without any conversation store: {}",
        error.message
    );
    assert!(
        !empty_home.path().join(".gemini").exists(),
        "the refused fork did not create the Antigravity store"
    );
}

#[test]
fn a_foreign_scope_file_cannot_stand_in_for_the_exact_conversation() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let index = open_index(home.path(), UP_TO_DATE);
    insert(
        &index,
        ROOT_ID,
        "Fix the login redirect",
        "",
        "2026-11-18 09:32:12.5207842+00:00",
        project.path(),
        "",
        0,
        "antigravity-cli",
    );
    drop(index);
    conversation_store(home.path(), ROOT_ID);

    let host = crate::host::local::new();
    let mut resume = request(
        &*host,
        home.path(),
        project.path(),
        ROOT_ID,
        SessionOperationKind::Resume,
    );
    resume.scope.session_file = Some(PathBuf::from("/some/other/session.jsonl"));
    let error = host.session_operation(&resume).unwrap_err();
    assert_eq!(error.kind, SessionErrorKind::Identity);
    assert!(
        error.message.contains("does not belong"),
        "{}",
        error.message
    );
}
