//! Read installed package resources without invoking npm, git or an agent.
//! Package/configuration filters remain native-runtime policy: these disk
//! installations carry Unknown rather than claiming that they are loaded.
use super::*;
use yaml_serde::Value;

pub(super) fn installed_packages(
    host: &dyn Host,
    store: &Path,
    agent: HookAgent,
    roots: &mut Vec<Root>,
    problems: &mut Vec<String>,
) {
    let mut queue = vec![(store.to_path_buf(), 0)];
    let mut seen = HashSet::new();
    let mut budget = 2_000;
    while let Some((directory, depth)) = queue.pop() {
        if budget == 0 {
            problems.push(format!(
                "{}: installed package scan limit reached",
                store.display()
            ));
            break;
        }
        budget -= 1;
        let real = match host.canonicalize(&directory) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                problems.push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        if !seen.insert(real) {
            continue;
        }
        match host.read_file(&host.join(&directory, "package.json"), MAX_MARKDOWN) {
            Ok(bytes) => {
                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(manifest) => {
                        if agent == HookAgent::Pi {
                            let declared = manifest
                                .get("pi")
                                .and_then(|v| v.get("skills"))
                                .and_then(|v| v.as_array());
                            if let Some(declared) =
                                declared.filter(|list| list.iter().all(|v| v.is_string()))
                            {
                                for value in declared.iter().filter_map(|v| v.as_str()) {
                                    manifest_entry(host, &directory, value, agent, roots, problems);
                                }
                            } else {
                                root(roots, host.join(&directory, "skills"), agent);
                            }
                        } else {
                            root(roots, host.join(&directory, "skills"), agent);
                        }
                    }
                    Err(_) => {
                        problems.push(format!("{}: invalid package.json", directory.display()))
                    }
                }
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                problems.push(format!("{}: {error}", directory.display()));
                continue;
            }
        }
        if depth >= 6 {
            continue;
        }
        match host.read_dir(&directory, None) {
            Ok(entries) => {
                for entry in entries.into_iter().rev().filter(|entry| {
                    entry.is_dir && !entry.name.starts_with('.') && entry.name != "node_modules"
                }) {
                    queue.push((host.join(&directory, &entry.name), depth + 1));
                }
            }
            Err(error) => problems.push(format!("{}: {error}", directory.display())),
        }
    }
}
pub(super) fn local_packages(
    host: &dyn Host,
    base: &Path,
    settings: &Value,
    agent: HookAgent,
    roots: &mut Vec<Root>,
    problems: &mut Vec<String>,
) {
    for entry in settings
        .get("packages")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
    {
        let source = entry
            .as_str()
            .or_else(|| entry.get("source").and_then(Value::as_str));
        let Some(source) = source else { continue };
        if source.starts_with("npm:")
            || source.starts_with("git:")
            || source.starts_with("https:")
            || source.starts_with("ssh:")
        {
            continue;
        }
        let path = if host.is_absolute(Path::new(source)) {
            PathBuf::from(source)
        } else {
            host.join(base, source)
        };
        let before = roots.len();
        installed_packages(host, &path, agent, roots, problems);
        if entry
            .get("skills")
            .and_then(Value::as_sequence)
            .is_some_and(|v| v.is_empty())
        {
            for root in roots.iter_mut().skip(before) {
                root.state = DiscoveryState::Disabled;
            }
        }
    }
}
pub(super) fn skillshare(
    host: &dyn Host,
    agent_dir: &Path,
    project_dirs: &[PathBuf],
    roots: &mut Vec<Root>,
    problems: &mut Vec<String>,
) {
    let store = host.join(&host.join(agent_dir, ".."), "skillshare");
    let locks = std::iter::once(host.join(agent_dir, "skills.lock.json")).chain(
        project_dirs
            .iter()
            .map(|project| under(host, project, &[".omp", "skills.lock.json"])),
    );
    for lock in locks {
        let bytes = match host.read_file(&lock, MAX_MARKDOWN) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                problems.push(format!("{}: {error}", lock.display()));
                continue;
            }
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            problems.push(format!("{}: invalid Skillshare lock", lock.display()));
            continue;
        };
        if value.get("version").and_then(|v| v.as_u64()) != Some(1) {
            problems.push(format!(
                "{}: unsupported Skillshare lock version",
                lock.display()
            ));
            continue;
        }
        for (id, entry) in value
            .get("skills")
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
        {
            let valid_id = id.starts_with('@')
                && id.split('/').count() == 2
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"@/_-".contains(&b));
            let version = entry.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if !valid_id
                || version.is_empty()
                || version.contains("..")
                || version.contains(['/', '\\'])
            {
                problems.push(format!("{}: invalid Skillshare identity", lock.display()));
                continue;
            }
            let path = host.join(&host.join(&store, id), version);
            let expected = entry.get("integrity").and_then(|v| v.as_str());
            let valid = host
                .read_file(&host.join(&path, ".skillshare-integrity"), 4096)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .is_some_and(|value| Some(value.trim()) == expected);
            root(roots, path, HookAgent::OhMyPi);
            if !valid {
                problems.push(format!("{}: Skillshare integrity marker does not match its lock; native loading is not verified",lock.display()));
            }
        }
    }
}
fn root(roots: &mut Vec<Root>, path: PathBuf, agent: HookAgent) {
    roots.push(Root {
        path,
        agent,
        kind: SourceKind::AgentPrivate,
        state: DiscoveryState::Unknown,
        recursive: agent == HookAgent::Pi,
        flat_markdown: agent == HookAgent::Pi,
    });
}
fn manifest_entry(
    host: &dyn Host,
    base: &Path,
    value: &str,
    agent: HookAgent,
    roots: &mut Vec<Root>,
    problems: &mut Vec<String>,
) {
    if value.starts_with(['!', '-']) {
        return;
    }
    let value = value.trim_start_matches('+');
    if !value.contains(['*', '?']) {
        root(roots, host.join(base, value), agent);
        return;
    }
    // Only expand inside the installed package. Never traverse a dependency's
    // node_modules or follow a link while expanding an untrusted glob.
    let pattern = glob_regex(value);
    let Ok(pattern) = regex::Regex::new(&pattern) else {
        problems.push(format!(
            "{}: invalid skill resource pattern",
            base.display()
        ));
        return;
    };
    let mut queue = vec![(base.to_path_buf(), String::new(), 0)];
    let mut budget = 2_000;
    while let Some((directory, relative, depth)) = queue.pop() {
        if budget == 0 {
            problems.push(format!(
                "{}: skill manifest expansion limit reached",
                base.display()
            ));
            break;
        }
        budget -= 1;
        let entries = match host.read_dir(&directory, None) {
            Ok(entries) => entries,
            Err(error) => {
                problems.push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        for entry in entries.into_iter().filter(|entry| {
            !entry.is_symlink && !entry.name.starts_with('.') && entry.name != "node_modules"
        }) {
            let relative = if relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{relative}/{}", entry.name)
            };
            let path = host.join(&directory, &entry.name);
            if pattern.is_match(&relative) {
                root(roots, path.clone(), agent);
            }
            if entry.is_dir && depth < MAX_DEPTH {
                queue.push((path, relative, depth + 1));
            }
        }
    }
}
fn glob_regex(glob: &str) -> String {
    let mut result = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        result.push_str("(?:.*/)?");
                    } else {
                        result.push_str(".*");
                    }
                } else {
                    result.push_str("[^/]*");
                }
            }
            '?' => result.push_str("[^/]"),
            _ => result.push_str(&regex::escape(&ch.to_string())),
        }
    }
    result.push('$');
    result
}
