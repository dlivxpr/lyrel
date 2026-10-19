use super::*;
use crate::core::agent_hooks::{HookAgent, HookTarget};
use serde_json::Value;

/// OMP 18.12.1 effective scope, resolved only through the selected Host.
pub fn discover_omp(host: &dyn Host, launch: &HashMap<String, String>) -> io::Result<Vec<Source>> {
    let environment = host.agent_environment()?;
    let home = [
        if host.separator() == '\\' {
            "USERPROFILE"
        } else {
            "HOME"
        },
        if host.separator() == '\\' {
            "HOME"
        } else {
            "USERPROFILE"
        },
    ]
    .into_iter()
    .find_map(|k| environment.get(k).filter(|v| !v.is_empty()))
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "selected Host does not report home; upgrade lyrel-server",
        )
    })?;
    let target = HookTarget::remote(host, home.into()).with_launch_commands(launch);
    let environment = target.agent_environment(HookAgent::OhMyPi)?;
    let home = target.agent_home(&environment)?;
    let profile = crate::core::agent_history::omp_profile(&environment)?;
    let config = environment
        .get("PI_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map_or(".omp", String::as_str);
    let base = if host.is_absolute(std::path::Path::new(config)) {
        PathBuf::from(config)
    } else {
        host.join(&home, config)
    };
    let root = match profile {
        Some(profile) => host.join(&host.join(&host.join(&base, "profiles"), profile), "agent"),
        None => environment
            .get("PI_CODING_AGENT_DIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| host.join(&base, "agent"), PathBuf::from),
    };
    if !host.is_absolute(&root) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "relative OMP agent directory requires native working-directory scope",
        ));
    }
    let scope = format!(
        "omp:{}:{}",
        profile.unwrap_or("default"),
        root.to_string_lossy()
    );
    let category = |key: &str| -> io::Result<PathBuf> {
        if host.separator() == '/' && (profile.is_some() || root == host.join(&base, "agent")) {
            if let Some(xdg) = environment.get(key).filter(|v| !v.is_empty()) {
                let mut path = host.join(std::path::Path::new(xdg), "omp");
                if let Some(profile) = profile {
                    path = host.join(&host.join(&path, "profiles"), profile);
                }
                match host.stat(&path) {
                    Ok(meta) if meta.is_dir => return Ok(path),
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
        }
        Ok(root.clone())
    };
    let mut sources = vec![Source::omp(
        host.join(&category("XDG_DATA_HOME")?, "sessions"),
        scope.clone(),
    )];
    if let Some(directory) = target.omp_session_dir()? {
        let directory = PathBuf::from(directory);
        if !host.is_absolute(&directory) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "relative OMP --session-dir requires native working-directory scope",
            ));
        }
        sources.push(Source::omp(directory, scope.clone()));
    }
    // Durable custom-file registry and terminal breadcrumbs include histories launched outside lyrel.
    for (directory, breadcrumb) in [("custom-session-files", false), ("terminal-sessions", true)] {
        let directory = host.join(&category("XDG_STATE_HOME")?, directory);
        let entries = match host.read_dir(&directory, None) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if entries.len() > MAX_FILES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "OMP discovery marker limit reached",
            ));
        }
        for entry in entries {
            if entry.is_dir || entry.is_symlink {
                continue;
            }
            let path = host.join(&directory, &entry.name);
            let bytes = host.read_file(&path, 64 * 1024)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid OMP history marker")
            })?;
            let file = if breadcrumb {
                text.lines().nth(1)
            } else {
                Some(text.trim())
            };
            if let Some(file) = file.filter(|f| f.ends_with(".jsonl")) {
                let file = PathBuf::from(file);
                if host.is_absolute(&file) && host.stat(&file).is_ok() {
                    let artifacts =
                        PathBuf::from(file.to_string_lossy().trim_end_matches(".jsonl"));
                    sources.push(Source::omp(file, scope.clone()));
                    if host.stat(&artifacts).is_ok_and(|m| m.is_dir) {
                        sources.push(Source::omp(artifacts, scope.clone()));
                    }
                }
            }
        }
    }
    Ok(sources)
}

pub(super) fn parse(bytes: &[u8]) -> io::Result<NativeSession> {
    let error = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "unreadable or unsupported OMP native history",
        )
    };
    let mut header = None;
    let mut records = Vec::new();
    for line in bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        let value: Value = serde_json::from_slice(line).map_err(|_| error())?;
        match value.get("type").and_then(Value::as_str) {
            Some("title") => {}
            Some("session") => {
                if header.is_some()
                    || value
                        .get("version")
                        .and_then(Value::as_u64)
                        .is_some_and(|v| v > 3)
                {
                    return Err(error());
                }
                header = Some(NativeSession {
                    id: value
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(error)?
                        .into(),
                    parent: value
                        .get("parentSession")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    timestamp: timestamp(&value).ok_or_else(error)?,
                    project: value.get("cwd").and_then(Value::as_str).map(str::to_owned),
                    records: Vec::new(),
                });
            }
            Some("message" | "model_usage") => {
                let payload = if value["type"] == "model_usage" {
                    &value
                } else {
                    let m = &value["message"];
                    if m["role"] != "assistant" {
                        continue;
                    }
                    m
                };
                let u = &payload["usage"];
                // An assistant without usage is an unknown call, not a free call.
                let count = |key: &str| u.get(key).and_then(Value::as_u64);
                let input = count("input");
                let output = count("output");
                let cache_read = count("cacheRead");
                let cache_write = count("cacheWrite");
                let total = count("totalTokens").or_else(|| {
                    input?
                        .checked_add(output?)?
                        .checked_add(cache_read?)?
                        .checked_add(cache_write?)
                });
                let recorded_usd = u
                    .get("cost")
                    .and_then(|c| c.get("total"))
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && *v >= 0.);
                records.push(UsageRecord {
                    id: value
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|v| !v.is_empty())
                        .ok_or_else(error)?
                        .into(),
                    timestamp: timestamp(&value).ok_or_else(error)?,
                    provider: payload
                        .get("provider")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    model: payload
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    tokens: Tokens {
                        input,
                        output,
                        cache_read,
                        cache_write,
                        total,
                    },
                    recorded_usd,
                    coverage_start: None,
                    estimated_usd: None,
                    time_unknown: false,
                });
            }
            _ => {}
        }
    }
    let mut header = header.ok_or_else(error)?;
    header.records = records;
    Ok(header)
}
fn timestamp(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?)
        .ok()
        .map(|d| d.timestamp())
}
