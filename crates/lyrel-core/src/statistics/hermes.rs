//! Hermes 0.22.2 public launcher/gateway adapter. No database or Python guessing.
use super::*;
use crate::core::agent_hooks::{HookAgent, HookTarget};
use crate::core::cli_agent::{CLIAgent, parse_command_argv};
use crate::host::session_operations::{SessionOperation, SessionOperationKind, SessionScope};
use serde_json::Value;

impl Source {
    pub fn hermes(root: PathBuf, scope: impl Into<String>, request: SessionOperation) -> Self {
        Self {
            root,
            scope: scope.into(),
            agent: "hermes".into(),
            hermes_request: Some(std::sync::Arc::new(request)),
            opencode_request: None,
            max_depth: None,
            discovery_issue: None,
        }
    }
}

pub fn discover_hermes(
    host: &dyn Host,
    launch: &HashMap<String, String>,
) -> io::Result<Vec<Source>> {
    let environment = host.agent_environment()?;
    let home = if host.separator() == '\\' {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    }
    .into_iter()
    .find_map(|key| environment.get(key).filter(|v| !v.is_empty()))
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Hermes Host home unavailable; update lyrel-server",
        )
    })?;
    let target = HookTarget::remote(host, home.into()).with_launch_commands(launch);
    let root = target.hermes_home()?;
    let environment = target.agent_environment(HookAgent::Hermes)?;
    let cwd = target.agent_home(&environment)?;
    let argv = parse_command_argv(
        launch
            .get("hermes")
            .filter(|v| !v.trim().is_empty())
            .map_or("hermes", String::as_str),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let mut scope = SessionScope::default();
    scope.environment = environment
        .into_iter()
        .filter(|(key, _)| {
            ["HOME", "USERPROFILE", "HERMES_HOME", "HERMES_RUNTIME_DIR"].contains(&key.as_str())
        })
        .collect();
    scope
        .environment
        .insert("HERMES_HOME".into(), root.to_string_lossy().into_owned());
    let normalized = root.to_string_lossy();
    let mut parts = normalized.trim_end_matches(['/', '\\']).rsplit(['/', '\\']);
    let name = parts.next().unwrap_or("");
    scope.profile = Some(
        if parts.next() == Some("profiles") {
            name
        } else {
            "default"
        }
        .into(),
    );
    let identity = format!("hermes:{}", root.to_string_lossy());
    Ok(vec![Source::hermes(
        root,
        identity,
        SessionOperation {
            expected_host: host.id().0,
            agent: CLIAgent::Hermes,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd,
            launch_argv: argv,
            scope,
        },
    )])
}

pub(super) fn scan_source(
    host: &dyn Host,
    source: &Source,
    cache: &mut StatisticsCache,
    snapshot: &mut Snapshot,
) {
    let key = (host.id().0, source.agent.clone(), source.scope.clone());
    let result = source
        .hermes_request
        .as_deref()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Hermes source has no launcher scope",
            )
        })
        .and_then(|request| host.hermes_statistics(request).map_err(io::Error::other))
        .and_then(|result| parse(source, &result, snapshot.updated));
    match result {
        Ok((files, issues)) => {
            let ambiguous_empty = files.is_empty()
                && issues
                    .iter()
                    .any(|issue| issue.reason.starts_with("Hermes empty response"));
            if ambiguous_empty {
                if let Some(old) = cache.native.get(&key) {
                    snapshot.files.extend(old.iter().cloned());
                }
            } else {
                cache.native.insert(key, files.clone());
                snapshot.files.extend(files);
            }
            snapshot.issues.extend(issues);
        }
        Err(error) => {
            snapshot.issues.push(ScanIssue {
                path: source.root.clone(),
                reason: error.to_string(),
            });
            if let Some(files) = cache.native.get(&key) {
                snapshot.files.extend(files.iter().cloned());
            }
        }
    }
}

fn parse(
    source: &Source,
    value: &Value,
    observed: i64,
) -> io::Result<(Vec<FileFacts>, Vec<ScanIssue>)> {
    let projects = value["projects"]
        .as_array()
        .ok_or_else(|| io::Error::other("Hermes projects missing"))?;
    let scoped = value["scoped_session_ids"]
        .as_array()
        .ok_or_else(|| io::Error::other("Hermes scoped_session_ids missing"))?;
    let issue = |reason: &str| ScanIssue {
        path: source.root.clone(),
        reason: reason.into(),
    };
    let mut issues = vec![issue(
        "Hermes public projects.tree excludes children usage, archived and zero-message sessions; totals cover visible native CLI history only",
    )];
    if projects.is_empty() && scoped.is_empty() {
        issues.push(issue("Hermes empty response cannot distinguish empty history from an unavailable profile database"));
    }
    if scoped.len() >= 501 {
        issues.push(issue("Hermes global 501-session window reached; public API has no offset/cursor, older history is unavailable"));
    }
    let mut files = Vec::new();
    let mut ids = HashSet::new();
    let mut lineage_members = HashSet::new();
    let mut metadata_count = 0;
    for project in projects {
        let rows = project["previewSessions"]
            .as_array()
            .ok_or_else(|| io::Error::other("Hermes project previewSessions missing"))?;
        metadata_count += rows.len();
        if rows.len() >= 501
            || project["sessionCount"]
                .as_u64()
                .is_some_and(|n| n > rows.len() as u64)
            || project["sessionIds"]
                .as_array()
                .is_some_and(|ids| ids.len() > rows.len())
        {
            issues.push(issue(
                "Hermes project preview window is incomplete; no offset/cursor is available",
            ));
        }
        for row in rows {
            if row["source"] != "cli" || row["archived"] == true {
                continue;
            }
            let id = row["id"]
                .as_str()
                .filter(|id| {
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                })
                .ok_or_else(|| {
                    io::Error::other("Hermes returned invalid stored session identity")
                })?;
            if !ids.insert(id.to_owned()) {
                continue;
            }
            if let Some(profile) = row["profile"].as_str() {
                if source
                    .hermes_request
                    .as_ref()
                    .and_then(|r| r.scope.profile.as_deref())
                    .is_some_and(|p| p != profile)
                {
                    issues.push(issue(
                        "Hermes response profile differs from selected scope; row omitted",
                    ));
                    continue;
                }
            }
            // Rolled-up compression lineage must never be billed twice. Branch rows retain their independent stored ID.
            if lineage_members.contains(id) {
                issues.push(issue(
                    "Hermes overlapping native lineage; duplicate cumulative usage omitted",
                ));
                continue;
            }
            if let Some(members) = row["_lineage_ids"].as_array() {
                if members
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|id| lineage_members.contains(id))
                {
                    issues.push(issue(
                        "Hermes overlapping native lineage; duplicate cumulative usage omitted",
                    ));
                    continue;
                }
                lineage_members.extend(members.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            lineage_members.insert(id.to_owned());
            let seconds = |key: &str| {
                row[key]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0. && *v < i64::MAX as f64)
                    .map(|v| v as i64)
            };
            let end = seconds("last_active");
            let start = seconds("started_at");
            let interval = start.zip(end).filter(|(s, e)| s <= e);
            if interval.is_none() {
                issues.push(issue("Hermes stored usage has no reliable activity interval; cumulative usage cannot be assigned to a date range"));
            }
            let input = row["input_tokens"].as_u64();
            let output = row["output_tokens"].as_u64();
            let cost = |key: &str| row[key].as_f64().filter(|v| v.is_finite() && *v >= 0.);
            let records = vec![UsageRecord {
                id: format!("stored:{id}"),
                fingerprint: None,
                timestamp: end.or(start).unwrap_or(i64::MIN),
                coverage_start: interval.map(|(start, _)| start),
                time_unknown: interval.is_none(),
                provider: String::new(),
                model: row["model"].as_str().unwrap_or("").into(),
                tokens: Tokens {
                    input,
                    output,
                    total: input.zip(output).and_then(|(a, b)| a.checked_add(b)),
                    ..Default::default()
                },
                recorded_usd: cost("actual_cost_usd"),
                estimated_usd: cost("estimated_cost_usd"),
            }];
            let project_path = if project["isNoProject"] == true {
                None
            } else {
                project["path"]
                    .as_str()
                    .filter(|p| !p.is_empty())
                    .or_else(|| row["git_repo_root"].as_str().filter(|p| !p.is_empty()))
                    .map(str::to_owned)
            };
            files.push(FileFacts {
                source: source.clone(),
                path: source.root.join(id),
                child: false,
                observed,
                metadata_stamp: None,
                session: NativeSession {
                    id: id.into(),
                    parent: None,
                    timestamp: end.or(start).unwrap_or(i64::MIN),
                    project: project_path,
                    records,
                    activity: Vec::new(),
                },
            });
        }
    }
    if metadata_count >= 501 && scoped.len() < 501 {
        issues.push(issue(
            "Hermes global 501-session metadata window reached; public API has no offset/cursor",
        ));
    }
    Ok((files, issues))
}
