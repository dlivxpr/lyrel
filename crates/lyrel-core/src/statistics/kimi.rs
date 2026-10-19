//! Kimi Code 2.2.0 / protocol 1.5 durable usage. `usageScope` labels the
//! request source, not accumulation: UsageAgentModel.record folds both scopes.
use super::*;
use crate::core::agent_hooks::{HookAgent, HookTarget};
use serde_json::Value;
use std::hash::{Hash, Hasher};

pub fn discover_kimi(host: &dyn Host, launch: &HashMap<String, String>) -> io::Result<Vec<Source>> {
    let environment = host.agent_environment()?;
    let keys = if host.separator() == '\\' {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    let home = keys
        .into_iter()
        .find_map(|key| environment.get(key).filter(|v| !v.is_empty()))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "selected Host does not report home; upgrade lyrel-server",
            )
        })?;
    let target = HookTarget::remote(host, home.into()).with_launch_commands(launch);
    let environment = target.agent_environment(HookAgent::Kimi)?;
    let home = match environment.get("KIMI_CODE_HOME") {
        Some(home) => PathBuf::from(home),
        None => host.join(&target.agent_home(&environment)?, ".kimi-code"),
    };
    if !host.is_absolute(&home) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "empty or relative KIMI_CODE_HOME requires native working-directory context; no default-home fallback",
        ));
    }
    let home = match host.canonicalize(&home) {
        Ok(home) => home,
        Err(error) if error.kind() == io::ErrorKind::NotFound => home,
        Err(error) => return Err(error),
    };
    Ok(vec![Source::kimi(
        host.join(&home, "sessions"),
        format!("kimi:{}", home.display()),
    )])
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
fn issue(snapshot: &mut Snapshot, path: &std::path::Path, reason: impl ToString) {
    snapshot.issues.push(ScanIssue {
        path: path.into(),
        reason: reason.to_string(),
    });
}
fn directories(
    host: &dyn Host,
    path: &std::path::Path,
    snapshot: &mut Snapshot,
    dirs: &mut usize,
) -> Vec<PathBuf> {
    *dirs += 1;
    if *dirs > MAX_DIRS {
        issue(snapshot, path, "directory scan limit reached");
        return Vec::new();
    }
    if host.stat(path).is_ok_and(|meta| meta.is_symlink) {
        issue(
            snapshot,
            path,
            "symbolic link omitted from Kimi statistics scope",
        );
        return Vec::new();
    }
    match host.read_dir(path, None) {
        Ok(entries) => entries
            .into_iter()
            .filter_map(|entry| {
                let entry_path = host.join(path, &entry.name);
                if entry.is_symlink {
                    issue(
                        snapshot,
                        &entry_path,
                        "symbolic link omitted from Kimi statistics scope",
                    );
                    None
                } else {
                    entry.is_dir.then_some(entry_path)
                }
            })
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            issue(snapshot, path, error);
            Vec::new()
        }
    }
}

pub(super) fn scan_source(
    host: &dyn Host,
    source: &Source,
    cache: &mut StatisticsCache,
    snapshot: &mut Snapshot,
    seen: &mut HashSet<(String, String, PathBuf)>,
    dirs: &mut usize,
    files: &mut usize,
    bytes: &mut u64,
) {
    for workspace in directories(host, &source.root, snapshot, dirs) {
        for directory in directories(host, &workspace, snapshot, dirs) {
            let result = (|| -> io::Result<()> {
                let name = directory.to_string_lossy();
                let name = name.rsplit(['/', '\\']).next().unwrap_or("");
                if !name.starts_with("session_") {
                    return Ok(());
                }
                let direct = host.join(&directory, "state.json");
                let state_path = match host.stat(&direct) {
                    Ok(meta) if meta.is_symlink => {
                        return Err(invalid("symbolic link omitted from Kimi session metadata"));
                    }
                    Ok(_) => direct,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        host.join(&host.join(&directory, "session-meta"), "state.json")
                    }
                    Err(error) => return Err(error),
                };
                if host.stat(&state_path)?.is_symlink {
                    return Err(invalid("symbolic link omitted from Kimi session metadata"));
                }
                let state_bytes = host.read_file(&state_path, 256 * 1024)?;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                state_bytes.hash(&mut hasher);
                let metadata_stamp = Some(hasher.finish());
                *bytes += state_bytes.len() as u64;
                if *bytes > MAX_SCAN_BYTES {
                    return Err(invalid("statistics byte scan limit reached"));
                }
                let state: Value = serde_json::from_slice(&state_bytes)
                    .map_err(|_| invalid("invalid Kimi session metadata"))?;
                if state["id"].as_str() != Some(name) {
                    return Err(invalid(
                        "Kimi session directory and native identity disagree",
                    ));
                }
                if state["archived"] == true {
                    let child_prefix = format!("{name}::");
                    cache.files.retain(|(id, agent, scope, _), (_, facts)| {
                        *id != host.id().0
                            || *agent != source.agent
                            || *scope != source.scope
                            || (facts.session.id != name
                                && !facts.session.id.starts_with(&child_prefix))
                    });
                    return Ok(());
                }
                let agents = host.join(&directory, "agents");
                let agent_dirs = directories(host, &agents, snapshot, dirs);
                if agent_dirs.is_empty() {
                    return Err(invalid(
                        "Kimi session has no readable durable agent history",
                    ));
                }
                for agent_dir in agent_dirs {
                    let path = host.join(&agent_dir, "wire.jsonl");
                    let agent_name = agent_dir.to_string_lossy();
                    let agent = agent_name.rsplit(['/', '\\']).next().unwrap_or("");
                    if !seen.insert((source.agent.clone(), source.scope.clone(), path.clone())) {
                        continue;
                    }
                    *files += 1;
                    if *files > MAX_FILES {
                        return Err(invalid("file scan limit reached"));
                    }
                    let key = (
                        host.id().0,
                        source.agent.clone(),
                        source.scope.clone(),
                        path.clone(),
                    );
                    let parsed = (|| {
                        let meta = host.stat(&path)?;
                        if meta.is_symlink {
                            return Err(invalid("symbolic link omitted from Kimi durable history"));
                        }
                        if let Some((old, facts)) = cache.files.get(&key) {
                            if old == &meta && facts.metadata_stamp == metadata_stamp {
                                return Ok((meta, facts.clone()));
                            }
                        }
                        if meta.len > MAX_FILE_BYTES || *bytes + meta.len > MAX_SCAN_BYTES {
                            return Err(invalid("statistics byte scan limit reached"));
                        }
                        // A metadata change invalidates fork/archive/identity facts even when the wire is unchanged.
                        *bytes += meta.len;
                        let data = host.read_file(&path, MAX_FILE_BYTES)?;
                        let (session, child) = parse(&data, &state, agent)?;
                        Ok((
                            meta,
                            FileFacts {
                                source: source.clone(),
                                path: path.clone(),
                                child,
                                observed: snapshot.updated,
                                session,
                                metadata_stamp,
                            },
                        ))
                    })();
                    match parsed {
                        Ok((meta, facts)) => {
                            cache.files.insert(key, (meta, facts.clone()));
                            snapshot.files.push(facts);
                        }
                        Err(error) => {
                            issue(snapshot, &path, error);
                            if let Some((_, facts)) = cache.files.get(&key) {
                                snapshot.files.push(facts.clone());
                            }
                        }
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                issue(snapshot, &directory, error);
            }
            if *files > MAX_FILES || *dirs > MAX_DIRS || *bytes > MAX_SCAN_BYTES {
                return;
            }
        }
    }
}

fn parse(bytes: &[u8], state: &Value, agent: &str) -> io::Result<(NativeSession, bool)> {
    let native_id = state["id"]
        .as_str()
        .ok_or_else(|| invalid("missing Kimi native session ID"))?;
    let inherited = state["forkedFrom"].as_str().is_some()
        || state["custom"]["parent_session_id"].as_str().is_some();
    let mut boundary = !inherited;
    let mut metadata = false;
    let mut records = Vec::new();
    let mut activity = Vec::new();
    let mut active = None;
    let mut offset = 0usize;
    for (line_ix, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let line_offset = offset;
        offset += line.len() + 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if line.len() > 256 * 1024 {
            return Err(invalid("Kimi durable line exceeds 256 KiB scan limit"));
        }
        let record: Value = serde_json::from_slice(line)
            .map_err(|_| invalid("invalid or truncated Kimi durable record"))?;
        if line_ix == 0 {
            if record["type"] != "metadata" || record["protocol_version"] != "1.5" {
                return Err(invalid(
                    "Kimi statistics requires durable protocol 1.5 metadata",
                ));
            }
            metadata = true;
            // A subagent journal born strictly after native fork creation cannot
            // contain the copied prefix. Equal/unknown timestamps remain restricted.
            if agent != "main"
                && record["created_at"]
                    .as_i64()
                    .zip(state["createdAt"].as_i64())
                    .is_some_and(|(wire, fork)| wire > fork)
            {
                boundary = true;
            }
            continue;
        }
        if record["agentId"].as_str().is_some_and(|id| id != agent) {
            return Err(invalid(
                "Kimi durable agent identity disagrees with storage path",
            ));
        }
        if record["type"] == "forked" {
            if record["agentId"].as_str() != Some(agent) {
                return Err(invalid("Kimi fork marker lacks reliable agent identity"));
            }
            records.clear();
            activity.clear();
            active = None;
            boundary = true;
            continue;
        }
        if !boundary {
            continue;
        }
        match record["type"].as_str() {
            Some("turn.prompt" | "turn.steer" | "usage.record") => {
                let time = record["time"]
                    .as_i64()
                    .filter(|v| *v >= 0)
                    .ok_or_else(|| invalid("Kimi activity lacks epoch-millisecond time"))?
                    / 1000;
                active = Some(active.map_or(time, |old: i64| old.min(time)));
                if record["type"] != "usage.record" {
                    activity.push(time);
                    continue;
                }
                if record["agentId"].as_str() != Some(agent) {
                    return Err(invalid("Kimi usage lacks reliable agent identity"));
                }
                if record
                    .get("usageScope")
                    .is_some_and(|scope| !matches!(scope.as_str(), Some("turn" | "session")))
                {
                    return Err(invalid("unsupported Kimi usage scope"));
                }
                let usage = &record["usage"];
                let input = usage["inputOther"].as_u64();
                let output = usage["output"].as_u64();
                let cache_read = usage["inputCacheRead"].as_u64();
                let cache_write = usage["inputCacheCreation"].as_u64();
                let total = input
                    .and_then(|n| n.checked_add(output?))
                    .and_then(|n| n.checked_add(cache_read?))
                    .and_then(|n| n.checked_add(cache_write?));
                records.push(UsageRecord {
                    id: format!("{agent}:{line_offset}"),
                    fingerprint: None,
                    timestamp: time,
                    provider: String::new(),
                    model: record["model"].as_str().unwrap_or("").into(),
                    tokens: Tokens {
                        input,
                        output,
                        reasoning: None,
                        cache_read,
                        cache_write,
                        total,
                    },
                    recorded_usd: None,
                    coverage_start: None,
                    estimated_usd: None,
                    time_unknown: false,
                });
            }
            _ => {}
        }
    }
    if !metadata {
        return Err(invalid("Kimi durable metadata is missing"));
    }
    if !boundary {
        return Err(invalid(
            "Kimi inherited usage cannot be verified: forked marker is missing",
        ));
    }
    Ok((
        NativeSession {
            id: if agent == "main" {
                native_id.into()
            } else {
                format!("{native_id}::{agent}")
            },
            parent: None,
            timestamp: active.unwrap_or(i64::MIN),
            project: state["cwd"]
                .as_str()
                .or_else(|| state["workDir"].as_str())
                .or_else(|| state["custom"]["cwd"].as_str())
                .map(str::to_owned),
            records,
            activity,
        },
        agent != "main"
            || state["custom"]["parent_session_id"].as_str().is_some()
            || active.is_none(),
    ))
}
