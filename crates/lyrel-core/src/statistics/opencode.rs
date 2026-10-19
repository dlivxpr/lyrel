use super::*;
use crate::core::cli_agent::{CLIAgent, parse_command_argv};
use crate::host::session_operations::{SessionOperation, SessionOperationKind, SessionScope};

impl Source {
    /// A live selector is used only for the next Host query, never serialized.
    pub fn opencode(root: PathBuf, scope: impl Into<String>, request: SessionOperation) -> Self {
        Self {
            root,
            scope: scope.into(),
            agent: "opencode".into(),
            opencode_request: Some(Box::new(request)),
            hermes_request: None,
            max_depth: None,
            discovery_issue: None,
        }
    }
}

pub fn discover_opencode(
    host: &dyn Host,
    launch: &HashMap<String, String>,
) -> io::Result<Vec<Source>> {
    let environment = host.agent_environment()?;
    let home = environment
        .get(if host.separator() == '\\' {
            "USERPROFILE"
        } else {
            "HOME"
        })
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "OpenCode Host home is unavailable; upgrade lyrel-server",
            )
        })?;
    let cwd = PathBuf::from(home);
    let argv = parse_command_argv(&CLIAgent::OpenCode.launch_command(launch))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    Ok(vec![Source::opencode(
        cwd.clone(),
        "opencode:configured",
        SessionOperation {
            expected_host: host.id().0,
            agent: CLIAgent::OpenCode,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd,
            launch_argv: argv,
            scope: SessionScope::default(),
        },
    )])
}

pub(super) fn scan_source(
    host: &dyn Host,
    source: &Source,
    snapshot: &mut Snapshot,
    cache: &mut StatisticsCache,
) {
    use sha2::Digest;
    let selector = source
        .opencode_request
        .as_ref()
        .map(|request| {
            let encoded = serde_json::to_vec(request).expect("session selectors serialize");
            use std::fmt::Write as _;
            let mut selector = String::with_capacity(source.scope.len() + 65);
            selector.push_str(&source.scope);
            selector.push(':');
            for byte in sha2::Sha256::digest(encoded) {
                write!(&mut selector, "{byte:02x}").expect("String write succeeds");
            }
            selector
        })
        .unwrap_or_else(|| source.scope.clone());
    let key = (host.id().0, selector);
    let result = source
        .opencode_request
        .as_ref()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "OpenCode runtime selector is unavailable",
            )
        })
        .and_then(|request| host.opencode_statistics(request).map_err(io::Error::other))
        .and_then(|payload| parse(source, payload, snapshot.updated));
    match result {
        Ok((verified, facts, issues)) => {
            snapshot.issues.extend(issues);
            cache
                .opencode
                .insert(key, (verified.clone(), facts.clone(), snapshot.updated));
            if let Some(displayed) = snapshot.sources.iter_mut().find(|candidate| {
                candidate.agent == source.agent && candidate.scope == source.scope
            }) {
                *displayed = verified;
            }
            snapshot.files.extend(facts);
        }
        Err(error) => {
            snapshot.issues.push(ScanIssue {
                path: source.root.clone(),
                reason: error.to_string(),
            });
            if let Some((verified, facts, observed)) = cache.opencode.get(&key) {
                snapshot.updated = snapshot.updated.min(*observed);
                if let Some(displayed) = snapshot.sources.iter_mut().find(|candidate| {
                    candidate.agent == source.agent && candidate.scope == source.scope
                }) {
                    *displayed = verified.clone();
                }
                snapshot.files.extend(facts.clone());
            }
        }
    }
}

pub(super) fn parse(
    source: &Source,
    payload: crate::host::session_operations::opencode::OpenCodeStatistics,
    observed: i64,
) -> io::Result<(Source, Vec<FileFacts>, Vec<ScanIssue>)> {
    let mut source = source.clone();
    source.scope = payload.identity;
    source.opencode_request = None;
    let mut issues: Vec<_> = payload
        .issues
        .into_iter()
        .map(|reason| ScanIssue {
            path: source.root.clone(),
            reason,
        })
        .collect();
    let mut facts = Vec::new();
    let mut seen = HashSet::new();
    for item in payload.sessions {
        let id = item["info"]["id"].as_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "OpenCode session identity missing",
            )
        })?;
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let info = &item["info"];
        let created = info["time"]["created"].as_i64().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "OpenCode session creation timestamp missing",
            )
        })?;
        let mut records = Vec::new();
        let mut activity = Vec::new();
        let mut messages = HashSet::new();
        for message in item["messages"].as_array().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "OpenCode message page missing")
        })? {
            let timestamp = message["time"]["created"].as_i64().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "OpenCode message timestamp missing",
                )
            })?;
            // Official v2 fork projector retains original message timestamps.
            if !info["fork"].is_null() && timestamp < created {
                continue;
            }
            let compaction = message["type"] == "compaction";
            if message["type"] != "assistant" && message["type"] != "user" && !compaction {
                continue;
            }
            let message_id = message["id"].as_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "OpenCode message identity missing",
                )
            })?;
            if !messages.insert(message_id.to_owned()) {
                continue;
            }
            if !compaction {
                activity.push(timestamp / 1000);
            }
            if message["type"] == "user" {
                continue;
            }
            if compaction {
                if message["tokens"].is_null() && message["cost"].is_null() {
                    continue;
                }
                issues.push(ScanIssue { path: source.root.clone(), reason: format!("OpenCode {id}/{message_id}: compaction usage is recorded, but public message creation time cannot verify its usage-event completion time; date allocation is incomplete") });
            }
            let usage = &message["tokens"];
            let input = usage["input"].as_u64();
            let output = usage["output"].as_u64();
            let reasoning = usage["reasoning"].as_u64();
            let cache_read = usage["cache"]["read"].as_u64();
            let cache_write = usage["cache"]["write"].as_u64();
            let total = [input, output, reasoning, cache_read, cache_write]
                .into_iter()
                .try_fold(0u64, |sum, value| sum.checked_add(value?));
            let recorded_usd = message["cost"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.);
            if total.is_none() || recorded_usd.is_none() {
                issues.push(ScanIssue {
                    path: source.root.clone(),
                    reason: format!(
                        "OpenCode {id}/{message_id}: native token/cost fields unavailable"
                    ),
                });
            }
            let model = &message["model"];
            let model_id = model["id"].as_str().unwrap_or("unknown");
            let model_id = model["variant"]
                .as_str()
                .map_or_else(|| model_id.to_owned(), |v| format!("{model_id}#{v}"));
            records.push(UsageRecord {
                id: message_id.into(),
                fingerprint: None,
                timestamp: timestamp / 1000,
                provider: model["providerID"].as_str().unwrap_or("unknown").into(),
                model: model_id,
                tokens: Tokens {
                    input,
                    output,
                    reasoning,
                    cache_read,
                    cache_write,
                    total,
                },
                recorded_usd,
                coverage_start: None,
                estimated_usd: None,
                time_unknown: compaction,
            });
        }
        facts.push(FileFacts {
            source: source.clone(),
            path: PathBuf::from(id),
            child: !info["parentID"].is_null(),
            observed,
            metadata_stamp: None,
            session: NativeSession {
                id: id.into(),
                parent: None,
                timestamp: i64::MIN,
                activity,
                project: info["projectID"].as_str().map(str::to_owned),
                records,
            },
        });
    }
    Ok((source, facts, issues))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host::local::LocalHost;
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn opencode_statistics_pages_forks_children_scopes_and_stale_cache_use_real_host_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("opencode");
        let write_json = |name: &str, value: Value| {
            std::fs::write(dir.path().join(name), serde_json::to_vec(&value).unwrap()).unwrap()
        };
        let info = |id: &str, created| json!({"id":id,"projectID":"proj_fixture","location":{"directory":dir.path()},"time":{"created":created}});
        let message = |id: &str, created, count| json!({"id":id,"type":"assistant","time":{"created":created},"model":{"providerID":"native","id":"model"},"tokens":{"input":count,"output":2,"reasoning":3,"cache":{"read":4,"write":5}},"cost":0.25,"content":[{"type":"text","text":"PRIVATE BODY"}]});
        let root = info("ses_root", 1000);
        let mut fork = info("ses_fork", 3000);
        fork["fork"] = json!({"sessionID":"ses_root"});
        let mut child = info("ses_child", 3000);
        child["parentID"] = json!("ses_root");
        write_json(
            "server.json",
            json!({"pid":123,"urls":["http://127.0.0.1:12345/"]}),
        );
        write_json(
            "stats.json",
            json!({"data":{"steps":3,"tokens":{"input":60,"output":6,"reasoning":9,"cache":{"read":12,"write":15}},"cost":0.75}}),
        );
        write_json(
            "first.json",
            json!({"data":[root.clone()],"cursor":{"next":"sessions-next"}}),
        );
        write_json("next.json", json!({"data":[root,fork,child],"cursor":{}}));
        write_json(
            "root.json",
            json!({"data":[message("msg_a",2000,10)],"cursor":{"next":"messages-next"}}),
        );
        write_json(
            "root-next.json",
            json!({"data":[message("msg_a",2000,10)],"cursor":{}}),
        );
        write_json(
            "fork.json",
            json!({"data":[message("msg_copy",2000,10),message("msg_new",4000,20)],"cursor":{}}),
        );
        write_json(
            "child.json",
            json!({"data":[message("msg_child",4000,30)],"cursor":{}}),
        );
        write_json("empty.json", json!({"data":[],"cursor":{}}));
        let script = format!(
            "#!/bin/sh\ncd '{}' || exit 1\ncase \"$2\" in\nserver.info) cat server.json;;\nexperimental.session.stats) cat stats.json;;\nsession.list) case \"$*\" in *cursor=sessions-next*) cat next.json;; *) cat first.json;; esac;;\nsession.message.list) case \"$*\" in *cursor=messages-next*) cat root-next.json;; *sessionID=ses_root*) cat root.json;; *sessionID=ses_fork*) cat fork.json;; *sessionID=ses_child*) cat child.json;; *sessionID=ses_empty*) cat empty.json;; *) exit 1;; esac;;\n*) exit 1;;\nesac\n",
            dir.path().display()
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let request = SessionOperation {
            expected_host: 0,
            agent: CLIAgent::OpenCode,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: dir.path().into(),
            launch_argv: vec![executable.to_string_lossy().into_owned()],
            scope: SessionScope::default(),
        };
        let source = Source::opencode(dir.path().into(), "selector-a", request.clone());
        let host = LocalHost::new();
        let mut cache = StatisticsCache::default();
        let snapshot = scan(&*host, &[source.clone()], &mut cache).unwrap();
        let totals = snapshot.aggregate(&DateWindow::all("UTC"));
        assert_eq!(totals.main_sessions(), 2);
        assert_eq!(totals.tokens().input(), Some(60));
        assert_eq!(totals.tokens().reasoning(), Some(9));
        assert_eq!(totals.tokens().total(), Some(102));
        assert_eq!(totals.recorded_usd(), Some(0.75));
        assert!(totals.is_complete(), "{:?}", snapshot.issues());
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("PRIVATE BODY"));
        assert!(!encoded.contains("launch_argv"));

        let mut other = request.clone();
        other
            .scope
            .environment
            .insert("HOME".into(), "/another-home".into());
        let both = scan(
            &*host,
            &[
                source.clone(),
                Source::opencode(dir.path().into(), "selector-b", other),
            ],
            &mut cache,
        )
        .unwrap();
        assert_eq!(
            both.aggregate(&DateWindow::all("UTC")).tokens().total(),
            Some(204)
        );
        assert_eq!(both.aggregate(&DateWindow::all("UTC")).main_sessions(), 4);
        let mut compaction = message("msg_compaction", 6000, 40);
        compaction["type"] = json!("compaction");
        write_json("root-next.json", json!({"data":[compaction],"cursor":{}}));
        write_json(
            "stats.json",
            json!({"data":{"steps":3,"tokens":{"input":100,"output":8,"reasoning":12,"cache":{"read":16,"write":20}},"cost":1.0}}),
        );
        let compacted = scan(&*host, &[source.clone()], &mut StatisticsCache::default()).unwrap();
        assert_eq!(
            compacted
                .aggregate(&DateWindow::all("UTC"))
                .tokens()
                .total(),
            Some(156)
        );
        assert_eq!(
            compacted.aggregate(&DateWindow::all("UTC")).recorded_usd(),
            Some(1.0)
        );
        assert_eq!(
            compacted.aggregate(&DateWindow::all("UTC")).main_sessions(),
            2
        );
        assert!(
            compacted
                .issues()
                .iter()
                .any(|issue| issue.reason().contains("completion time"))
        );

        let mut mismatched = request.clone();
        mismatched.scope.opencode =
            Some(crate::host::session_operations::opencode::OpenCodeScope {
                server: Some(crate::host::session_operations::opencode::OpenCodeServer {
                    pid: 999,
                    urls: vec!["http://127.0.0.1:12345/".into()],
                }),
                ..Default::default()
            });
        assert_eq!(
            host.opencode_statistics(&mismatched).unwrap_err().kind,
            crate::host::session_operations::SessionErrorKind::Identity
        );
        write_json("root-next.json", json!({"data":[],"cursor":{}}));
        write_json(
            "next.json",
            json!({"data":[info("ses_empty",5000)],"cursor":{}}),
        );
        write_json(
            "stats.json",
            json!({"data":{"steps":1,"tokens":{"input":10,"output":2,"reasoning":3,"cache":{"read":4,"write":5}},"cost":0.25}}),
        );
        let empty = scan(&*host, &[source.clone()], &mut StatisticsCache::default()).unwrap();
        assert_eq!(empty.aggregate(&DateWindow::all("UTC")).main_sessions(), 1);
        assert!(empty.is_complete(), "{:?}", empty.issues());
        // Restore official lineage metadata before checking restricted pagination.
        let mut fork = info("ses_fork", 3000);
        fork["fork"] = json!({"sessionID":"ses_root"});
        let mut child = info("ses_child", 3000);
        child["parentID"] = json!("ses_root");
        write_json("next.json", json!({"data":[fork,child],"cursor":{}}));
        write_json(
            "stats.json",
            json!({"data":{"steps":3,"tokens":{"input":60,"output":6,"reasoning":9,"cache":{"read":12,"write":15}},"cost":0.75}}),
        );
        write_json(
            "root-next.json",
            json!({"data":[],"cursor":{"next":"messages-next"}}),
        );
        let partial = scan(&*host, &[source.clone()], &mut cache).unwrap();
        assert!(!partial.is_complete());
        assert!(
            partial
                .issues()
                .iter()
                .any(|issue| issue.reason().contains("repeated message cursor"))
        );
        assert_eq!(
            partial.aggregate(&DateWindow::all("UTC")).tokens().total(),
            Some(102)
        );
        std::fs::write(&executable, "#!/bin/sh\nexit 1\n").unwrap();
        let stale = scan(&*host, &[source], &mut cache).unwrap();
        assert!(!stale.is_complete());
        assert_eq!(
            stale.aggregate(&DateWindow::all("UTC")).tokens().total(),
            Some(102)
        );

        let mut wrong = request;
        wrong.scope.opencode = Some(crate::host::session_operations::opencode::OpenCodeScope {
            server: Some(crate::host::session_operations::opencode::OpenCodeServer {
                pid: 999,
                urls: vec!["http://127.0.0.1:12345/".into()],
            }),
            ..Default::default()
        });
        let wrong_host = host
            .opencode_statistics(&SessionOperation {
                expected_host: 77,
                ..wrong
            })
            .unwrap_err();
        assert_eq!(
            wrong_host.kind,
            crate::host::session_operations::SessionErrorKind::WrongHost
        );
    }
}
