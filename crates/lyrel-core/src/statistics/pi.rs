use super::*;
use crate::core::agent_hooks::{HookAgent, HookTarget};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// pi 1.3.0 storage, evaluated on the selected Host; never uses omp profiles.
pub fn discover_pi(host: &dyn Host, launch: &HashMap<String, String>) -> io::Result<Vec<Source>> {
    let environment = host.agent_environment()?;
    let keys = if host.separator() == '\\' { ["USERPROFILE", "HOME"] } else { ["HOME", "USERPROFILE"] };
    let home = keys.into_iter().find_map(|k| environment.get(k).filter(|v| !v.is_empty()))
        .ok_or_else(|| unsupported("selected Host does not report home; upgrade lyrel-server"))?;
    let target = HookTarget::remote(host, home.into()).with_launch_commands(launch);
    let environment = target.agent_environment(HookAgent::Pi)?;
    let home = target.agent_home(&environment)?;
    let agent = target.pi_agent_dir_checked()?;
    let selected = match environment.get("PI_CODING_AGENT_SESSION_DIR").filter(|v| !v.is_empty()) {
        Some(value) => Some(value.clone()),
        None => read_setting(host, &host.join(&agent, "settings.json"))?,
    };
    let (root, depth) = match selected {
        Some(value) => (absolute(host, &home, &value)?, 0),
        None => (host.join(&agent, "sessions"), 1),
    };
    let mut sources = vec![source(host, &home, &agent, root, depth)];
    // Existing Host history exposes captured pane selectors, not a second statistics cache.
    // This is only discovery: all accounting reads complete JSONL files below those stores.
    let sessions = match host.agent_sessions() {
        Ok(sessions) => sessions,
        Err(error) => {
            let mut failed = source(host, &home, &agent, agent.clone(), 0);
            failed.discovery_issue = Some(format!("pi known-source metadata unavailable: {error}"));
            sources.push(failed);
            return Ok(sources);
        }
    };
    if sessions.len() >= 500 {
        let mut limited = source(host, &home, &agent, agent.clone(), 0);
        limited.discovery_issue = Some("known-source discovery reached the 500-session history window; additional pi stores may be unavailable".into());
        sources.push(limited);
    }
    for session in sessions {
        if session.agent != crate::core::cli_agent::CLIAgent::Pi { continue; }
        let Some(scope) = session.scope else { continue; };
        let failed_path = scope.session_dir.clone().unwrap_or_else(|| agent.clone());
        let candidate = (|| -> io::Result<Option<Source>> {
        let known_home = keys.into_iter().find_map(|k| scope.environment.get(k).filter(|v| !v.is_empty())).map_or_else(|| home.clone(), PathBuf::from);
        if !host.is_absolute(&known_home) { return Err(unsupported("known pi home requires an absolute Host path")); }
        let known_agent = match scope.environment.get("PI_CODING_AGENT_DIR").filter(|v| !v.is_empty()) {
            Some(value) => absolute(host, &known_home, value)?,
            None => host.join(&host.join(&known_home, ".pi"), "agent"),
        };
        if known_home != home || known_agent != agent { return Ok(None); }
        let directory = if let Some(directory) = scope.session_dir {
            directory
        } else {
            let project_setting = session.cwd.as_ref().map(|cwd| host.join(&host.join(cwd, ".pi"), "settings.json"));
            let selected = if let Some(value) = scope.environment.get("PI_CODING_AGENT_SESSION_DIR").filter(|v| !v.is_empty()) {
                Some(value.clone())
            } else if let Some(value) = match project_setting { Some(path) => read_setting(host, &path)?, None => None } {
                Some(value)
            } else {
                read_setting(host, &host.join(&known_agent, "settings.json"))?
            };
            match selected {
                Some(value) if !host.is_absolute(std::path::Path::new(&value)) && !value.starts_with('~') => {
                    host.join(session.cwd.as_deref().ok_or_else(|| unsupported("relative pi sessionDir requires captured native cwd"))?, &value)
                }
                Some(value) => absolute(host, &known_home, &value)?,
                None => host.join(&known_agent, "sessions"),
            }
        };
        if !host.is_absolute(&directory) { return Err(unsupported("known pi session directory requires captured native cwd")); }
        Ok(Some(source(host, &known_home, &known_agent, directory, 0)))
        })();
        match candidate {
            Ok(Some(candidate)) if !sources.iter().any(|s| s.root == candidate.root || (s.max_depth == Some(1) && candidate.root.to_string_lossy().starts_with(&format!("{}{}", s.root.display(), host.separator())))) => sources.push(candidate),
            Ok(_) => {},
            Err(error) => {
                let mut failed = source(host, &home, &agent, failed_path, 0);
                failed.discovery_issue = Some(error.to_string());
                sources.push(failed);
            }
        }
    }
    Ok(sources)
}
fn source(host: &dyn Host, home: &std::path::Path, agent: &std::path::Path, root: PathBuf, depth: u8) -> Source {
    let root = host.canonicalize(&root).unwrap_or(root);
    let mut source = Source::pi(root.clone(), format!("pi:{}:{}:{}", home.display(), agent.display(), root.display()));
    source.max_depth = Some(depth);
    source
}
fn unsupported(message: &str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, message) }
fn absolute(host: &dyn Host, home: &std::path::Path, value: &str) -> io::Result<PathBuf> {
    let path = if value == "~" { home.into() } else if value.starts_with("~/") || value.starts_with("~\\") { host.join(home, &value[2..]) } else { value.into() };
    if !host.is_absolute(&path) { return Err(unsupported("relative pi sessionDir requires captured native cwd; no default-store fallback")); }
    Ok(path)
}
fn read_setting(host: &dyn Host, path: &std::path::Path) -> io::Result<Option<String>> {
    let bytes = match host.read_file(path, 1024 * 1024) { Ok(bytes) => bytes, Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None), Err(error) => return Err(error) };
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| unsupported("unreadable pi settings.json"))?;
    match value.get("sessionDir") { None | Some(Value::Null) => Ok(None), Some(Value::String(value)) if value.is_empty() => Ok(None), Some(Value::String(value)) => Ok(Some(value.clone())), _ => Err(unsupported("invalid pi sessionDir setting")) }
}

pub(super) fn parse(bytes: &[u8]) -> io::Result<NativeSession> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "unreadable or unsupported pi native history");
    let mut lines = bytes.split(|b| *b == b'\n').filter(|line| !line.iter().all(u8::is_ascii_whitespace));
    let header: Value = serde_json::from_slice(lines.next().ok_or_else(invalid)?).map_err(|_| invalid())?;
    let version = header.get("version").and_then(Value::as_u64).unwrap_or(1);
    if header["type"] != "session" || !(1..=3).contains(&version) { return Err(invalid()); }
    let mut session = NativeSession {
        id: header.get("id").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or_else(invalid)?.into(),
        parent: header.get("parentSession").and_then(Value::as_str).map(str::to_owned),
        timestamp: timestamp(&header).ok_or_else(invalid)?,
        project: header.get("cwd").and_then(Value::as_str).map(str::to_owned),
        records: Vec::new(),
    };
    for (ix, line) in lines.enumerate() {
        let entry: Value = serde_json::from_slice(line).map_err(|_| invalid())?;
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = if kind == "message" {
            let message = &entry["message"];
            match message["role"].as_str() {
                Some("assistant") => message,
                Some("toolResult") if message.get("usage").is_some() => message,
                _ => continue,
            }
        } else if kind == "usage" || (matches!(kind, "compaction" | "branch_summary") && entry.get("usage").is_some()) { &entry } else { continue };
        let usage = &payload["usage"];
        let count = |key| usage.get(key).and_then(Value::as_u64);
        let input = count("input");
        let output = count("output");
        let cache_read = count("cacheRead");
        let cache_write = count("cacheWrite");
        // Match pi's addUsageToTotals: totalTokens is not authoritative for the session sum.
        // reasoning is included in output; cacheWrite1h is included in cacheWrite.
        let total = input.and_then(|v| v.checked_add(output?))
            .and_then(|v| v.checked_add(cache_read?)).and_then(|v| v.checked_add(cache_write?));
        let id = match entry.get("id").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            Some(id) => id.to_owned(),
            None if version == 1 && session.parent.is_none() => format!("legacy-line-{ix}"),
            None => return Err(unsupported("pi inherited legacy usage has no reliable native entry identity")),
        };
        session.records.push(UsageRecord {
            fingerprint: Some(fingerprint(if kind == "message" { payload } else { usage }, &entry["timestamp"], kind)?),
            id,
            timestamp: timestamp(&entry).or_else(|| payload.get("timestamp").and_then(Value::as_i64).map(|v| v / 1000)).ok_or_else(invalid)?,
            provider: payload.get("provider").and_then(Value::as_str).unwrap_or("").into(),
            model: payload.get("responseModel").or_else(|| payload.get("model")).and_then(Value::as_str).unwrap_or("").into(),
            tokens: Tokens { input, output, cache_read, cache_write, total },
            recorded_usd: usage.get("cost").and_then(|v| v.get("total")).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.),
        });
    }
    Ok(session)
}
fn timestamp(value: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?).ok().map(|v| v.timestamp())
}

struct Fingerprint(Sha256);
impl io::Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { self.0.update(bytes); Ok(bytes.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn fingerprint(payload: &Value, timestamp: &Value, kind: &str) -> io::Result<String> {
    let mut digest = Fingerprint(Sha256::new());
    serde_json::to_writer(&mut digest, &(kind, timestamp, payload))?;
    Ok(format!("{:x}", digest.0.finalize()))
}

/// Resolve only native path-linked inheritance. Unrelated short entry IDs never establish lineage.
pub(super) fn resolve_inheritance(snapshot: &mut Snapshot) {
    let mut removals: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut excluded = HashSet::new();
    for (ix, file) in snapshot.files.iter().enumerate().filter(|(_, f)| f.source.agent == "pi") {
        let mut parent = file.session.parent.as_deref();
        let mut visited = HashSet::from([file.path.as_path()]);
        let mut inherited: HashMap<&str, &UsageRecord> = HashMap::new();
        let mut problem = None;
        while let Some(path) = parent {
            let ancestor = snapshot.files.iter().find(|candidate| candidate.source.agent == "pi"
                && candidate.source.scope == file.source.scope && candidate.path.to_string_lossy() == path);
            let Some(ancestor) = ancestor else { problem = Some("parentSession is unavailable or outside the selected storage scope"); break; };
            if !visited.insert(ancestor.path.as_path()) || visited.len() > 128 { problem = Some("parentSession cycle or lineage depth limit"); break; }
            for record in &ancestor.session.records { inherited.insert(&record.id, record); }
            parent = ancestor.session.parent.as_deref();
        }
        if let Some(reason) = problem {
            snapshot.issues.push(ScanIssue { path: file.path.clone(), reason: format!("{reason}; inherited usage cannot be verified; {} usage records excluded", file.session.records.len()) });
            excluded.insert(ix);
            continue;
        }
        for record in &file.session.records {
            if let Some(original) = inherited.get(record.id.as_str()) {
                if *original == record {
                    removals.entry(ix).or_default().insert(record.id.clone());
                } else {
                    snapshot.issues.push(ScanIssue { path: file.path.clone(), reason: format!("pi entry identity conflict: {}; distinct usage retained", record.id) });
                }
            }
        }
    }
    for (ix, file) in snapshot.files.iter_mut().enumerate().filter(|(_, f)| f.source.agent == "pi") {
        if excluded.contains(&ix) { file.session.records.clear(); }
        else if let Some(ids) = removals.get(&ix) { file.session.records.retain(|r| !ids.contains(&r.id)); }
        // Shared visitors now receive only verified incremental usage, not a second lineage algorithm.
        file.session.parent = None;
    }
}
