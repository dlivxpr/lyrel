//! Native discovery contracts: omp 18.12.2, pi 1.3.0, Kimi Code 2.2.0,
//! OpenCode 2.0.32 and Hermes 0.22.2. See the settings usage guide for sources.
use super::*;
use yaml_serde::Value;

pub(super) fn agent_roots(
    host: &dyn Host,
    target: &HookTarget<'_>,
    request: &ScanRequest,
    agent: HookAgent,
    problems: &mut Vec<String>,
) -> io::Result<Vec<Root>> {
    let env = target.agent_environment(agent)?;
    let home = target.agent_home(&env)?;
    let argv = request
        .launch_commands
        .get(agent.slug())
        .map(|v| crate::core::cli_agent::parse_command_argv(v))
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or_default();
    let disabled = argv
        .iter()
        .any(|a| a == "--no-skills" || (agent == HookAgent::Pi && a == "-ns"));
    let mut roots = Vec::new();
    let mut settings = Value::Null;
    let project_dirs = project_ancestors(host, request.project.as_deref(), &home)?;
    match agent {
        HookAgent::Pi => {
            let dir = target.pi_agent_dir_checked()?;
            settings = read_config(host, &host.join(&dir, "settings.json"), problems);
            push(
                &mut roots,
                host.join(&dir, "skills"),
                agent,
                SourceKind::AgentPrivate,
                true,
                true,
                DiscoveryState::Discovered,
            );
            push(
                &mut roots,
                under(host, &home, &[".agents", "skills"]),
                agent,
                SourceKind::UserShared,
                true,
                false,
                DiscoveryState::Discovered,
            );
            pi_paths(host, &home, &dir, &settings, agent, &mut roots, problems);
            packages::installed_packages(
                host,
                &under(host, &dir, &["npm", "node_modules"]),
                agent,
                &mut roots,
                problems,
            );
            packages::installed_packages(
                host,
                &host.join(&dir, "git"),
                agent,
                &mut roots,
                problems,
            );
            packages::local_packages(host, &dir, &settings, agent, &mut roots, problems);
            for project in &project_dirs {
                for name in [".pi", ".agents"] {
                    push(
                        &mut roots,
                        under(host, project, &[name, "skills"]),
                        agent,
                        if name == ".agents" {
                            SourceKind::ProjectShared
                        } else {
                            SourceKind::AgentPrivate
                        },
                        true,
                        name == ".pi",
                        DiscoveryState::Unknown,
                    );
                }
            }
            if let Some(project) = &request.project {
                let dir = host.join(project, ".pi");
                let config = read_config(host, &host.join(&dir, "settings.json"), problems);
                pi_paths(host, &home, &dir, &config, agent, &mut roots, problems);
                packages::installed_packages(
                    host,
                    &under(host, &dir, &["npm", "node_modules"]),
                    agent,
                    &mut roots,
                    problems,
                );
                packages::installed_packages(
                    host,
                    &host.join(&dir, "git"),
                    agent,
                    &mut roots,
                    problems,
                );
                packages::local_packages(host, &dir, &config, agent, &mut roots, problems);
            }
            if disabled {
                for root in &mut roots {
                    root.state = DiscoveryState::Disabled;
                }
            }
            for value in flag_values(&argv, "--skill") {
                add_path(
                    host,
                    &home,
                    request.project.as_deref(),
                    value,
                    agent,
                    &mut roots,
                    problems,
                );
            }
        }
        HookAgent::OhMyPi => {
            let base = env
                .get("PI_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map_or_else(
                    || host.join(&home, ".omp"),
                    |value| {
                        if host.is_absolute(Path::new(value)) {
                            PathBuf::from(value)
                        } else {
                            host.join(&home, value)
                        }
                    },
                );
            let profile = env
                .get("OMP_PROFILE")
                .or_else(|| env.get("PI_PROFILE"))
                .map(String::as_str)
                .filter(|v| !v.is_empty())
                .unwrap_or("default");
            if profile.contains(['/', '\\']) || matches!(profile, "." | "..") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid omp profile",
                ));
            }
            let dir = if profile != "default" {
                under(host, &base, &["profiles", profile, "agent"])
            } else if let Some(value) = env.get("PI_CODING_AGENT_DIR").filter(|v| !v.is_empty()) {
                absolute_selector(host, &home, value)?
            } else {
                host.join(&base, "agent")
            };
            settings = read_config(host, &host.join(&dir, "config.yml"), problems);
            if let Some(project) = &request.project {
                merge(
                    &mut settings,
                    read_config(
                        host,
                        &under(host, project, &[".omp", "config.yml"]),
                        problems,
                    ),
                );
                merge(
                    &mut settings,
                    read_config(
                        host,
                        &under(host, project, &[".omp", "settings.json"]),
                        problems,
                    ),
                );
            }
            for value in flag_values(&argv, "--config") {
                let path = resolve_path(host, &home, request.project.as_deref(), value)?;
                merge(&mut settings, read_config(host, &path, problems));
            }
            let skill_config = settings.get("skills").cloned().unwrap_or(Value::Null);
            push(
                &mut roots,
                host.join(&dir, "skills"),
                agent,
                SourceKind::AgentPrivate,
                false,
                false,
                if bool_field(&skill_config, "enablePiUser") == Some(false) {
                    DiscoveryState::Disabled
                } else {
                    DiscoveryState::Discovered
                },
            );
            push(
                &mut roots,
                host.join(&dir, "managed-skills"),
                agent,
                SourceKind::AgentPrivate,
                false,
                false,
                DiscoveryState::Discovered,
            );
            let agents_user = bool_field(&skill_config, "enableAgentsUser") != Some(false);
            packages::skillshare(host, &dir, &project_dirs, &mut roots, problems);
            packages::installed_packages(
                host,
                &under(host, &base, &["plugins", "node_modules"]),
                agent,
                &mut roots,
                problems,
            );
            if let Some(xdg) = env.get("XDG_DATA_HOME") {
                let plugins = under(host, Path::new(xdg), &["omp", "plugins", "node_modules"]);
                if host.is_absolute(&plugins) {
                    packages::installed_packages(host, &plugins, agent, &mut roots, problems);
                }
            }
            for project in &project_dirs {
                packages::installed_packages(
                    host,
                    &under(host, project, &[".omp", "plugins", "node_modules"]),
                    agent,
                    &mut roots,
                    problems,
                );
            }
            for value in flag_values(&argv, "--plugin-dir") {
                let path = resolve_path(host, &home, request.project.as_deref(), value)?;
                push(
                    &mut roots,
                    host.join(&path, "skills"),
                    agent,
                    SourceKind::AgentPrivate,
                    false,
                    false,
                    DiscoveryState::Unknown,
                );
            }
            for name in [".agents", ".agent"] {
                push(
                    &mut roots,
                    under(host, &home, &[name, "skills"]),
                    agent,
                    SourceKind::UserShared,
                    false,
                    false,
                    if agents_user {
                        DiscoveryState::Discovered
                    } else {
                        DiscoveryState::Disabled
                    },
                );
            }
            for project in &project_dirs {
                for (name, kind, key) in [
                    (".omp", SourceKind::AgentPrivate, "enablePiProject"),
                    (".agents", SourceKind::ProjectShared, "enableAgentsProject"),
                    (".agent", SourceKind::ProjectShared, "enableAgentsProject"),
                    (".claude", SourceKind::AgentPrivate, "enableClaudeProject"),
                    (".codex", SourceKind::AgentPrivate, "enableCodexProject"),
                    (
                        ".opencode",
                        SourceKind::AgentPrivate,
                        "enableOpenCodeProject",
                    ),
                    (".github", SourceKind::AgentPrivate, "enableGithubProject"),
                ] {
                    push(
                        &mut roots,
                        under(host, project, &[name, "skills"]),
                        agent,
                        kind,
                        false,
                        false,
                        if bool_field(&skill_config, key) == Some(false) {
                            DiscoveryState::Disabled
                        } else {
                            DiscoveryState::Discovered
                        },
                    );
                }
            }
            for (name, key) in [
                (".claude", "enableClaudeUser"),
                (".codex", "enableCodexUser"),
            ] {
                let state = if bool_field(&skill_config, key) == Some(true) {
                    DiscoveryState::Discovered
                } else {
                    DiscoveryState::Disabled
                };
                let root = if name == ".claude" {
                    env.get("CLAUDE_CONFIG_DIR")
                        .map(|v| absolute_selector(host, &home, v))
                        .transpose()?
                        .unwrap_or_else(|| host.join(&home, name))
                } else {
                    host.join(&home, name)
                };
                push(
                    &mut roots,
                    host.join(&root, "skills"),
                    agent,
                    SourceKind::AgentPrivate,
                    false,
                    false,
                    state,
                );
            }
            for value in strings(skill_config.get("customDirectories")) {
                add_path(
                    host,
                    &home,
                    request.project.as_deref(),
                    value,
                    agent,
                    &mut roots,
                    problems,
                );
            }
            if bool_field(&skill_config, "enabled") == Some(false) {
                for root in &mut roots {
                    root.state = DiscoveryState::Disabled;
                }
            }
            // Provider/name glob policies can exclude an individual file even
            // while its source is on. Keep runtime status explicitly unknown.
            if ["ignoredSkills", "includeSkills"]
                .iter()
                .any(|key| skill_config.get(*key).is_some())
                || settings.get("disabledProviders").is_some()
                || settings.get("enabledProviders").is_some()
                || settings.get("disabledExtensions").is_some()
            {
                for root in &mut roots {
                    if root.state != DiscoveryState::Disabled
                        || (settings.get("enabledProviders").is_some()
                            && bool_field(&skill_config, "enabled") != Some(false))
                    {
                        root.state = DiscoveryState::Unknown;
                    }
                }
            }
        }
        HookAgent::Kimi => {
            let config = target.kimi_config_path_checked()?;
            let dir = config.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Kimi configuration has no parent",
                )
            })?;
            let text = read_optional(host, &config, problems);
            let extra = if let Some(text) = text {
                match text.parse::<toml_edit::DocumentMut>() {
                    Ok(doc) => doc
                        .get("extra_skill_dirs")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    Err(_) => {
                        problems.push(format!(
                            "{}: invalid native Kimi configuration",
                            config.display()
                        ));
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let overrides = flag_values(&argv, "--skills-dir");
            let state = if overrides.is_empty() {
                DiscoveryState::Discovered
            } else {
                DiscoveryState::Disabled
            };
            push(
                &mut roots,
                host.join(dir, "skills"),
                agent,
                SourceKind::AgentPrivate,
                false,
                true,
                state,
            );
            push(
                &mut roots,
                under(host, &home, &[".agents", "skills"]),
                agent,
                SourceKind::UserShared,
                false,
                false,
                state,
            );
            let project_root = project_dirs
                .last()
                .filter(|path| host.exists(&host.join(path, ".git")))
                .or(request.project.as_ref());
            if let Some(project) = project_root {
                for name in [".kimi-code", ".agents"] {
                    push(
                        &mut roots,
                        under(host, project, &[name, "skills"]),
                        agent,
                        if name == ".agents" {
                            SourceKind::ProjectShared
                        } else {
                            SourceKind::AgentPrivate
                        },
                        false,
                        name != ".agents",
                        state,
                    );
                }
            }
            for value in overrides
                .into_iter()
                .chain(extra.iter().map(String::as_str))
            {
                add_path(
                    host,
                    &home,
                    request.project.as_deref(),
                    value,
                    agent,
                    &mut roots,
                    problems,
                );
            }
        }
        HookAgent::OpenCode => {
            let config = env
                .get("OPENCODE_CONFIG_DIR")
                .map(|v| absolute_selector(host, &home, v))
                .transpose()?
                .unwrap_or_else(|| {
                    env.get("XDG_CONFIG_HOME")
                        .map(|v| host.join(Path::new(v), "opencode"))
                        .unwrap_or_else(|| under(host, &home, &[".config", "opencode"]))
                });
            if !host.is_absolute(&config) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "OpenCode configuration root is relative on the selected Host",
                ));
            }
            for (path, kind) in [
                (host.join(&config, "skills"), SourceKind::AgentPrivate),
                (
                    under(host, &home, &[".claude", "skills"]),
                    SourceKind::AgentPrivate,
                ),
                (
                    under(host, &home, &[".agents", "skills"]),
                    SourceKind::UserShared,
                ),
            ] {
                push(
                    &mut roots,
                    path,
                    agent,
                    kind,
                    true,
                    true,
                    DiscoveryState::Discovered,
                );
            }
            let mut configs = vec![
                host.join(&config, "opencode.json"),
                host.join(&config, "opencode.jsonc"),
            ];
            if let Some(value) = env.get("OPENCODE_CONFIG") {
                configs.push(absolute_selector(host, &home, value)?);
            }
            for project in &project_dirs {
                for name in [".opencode", ".claude", ".agents"] {
                    push(
                        &mut roots,
                        under(host, project, &[name, "skills"]),
                        agent,
                        if name == ".agents" {
                            SourceKind::ProjectShared
                        } else {
                            SourceKind::AgentPrivate
                        },
                        true,
                        true,
                        DiscoveryState::Discovered,
                    );
                }
                for name in [
                    "opencode.json",
                    "opencode.jsonc",
                    ".opencode/opencode.json",
                    ".opencode/opencode.jsonc",
                ] {
                    configs.push(host.join(project, name));
                }
            }
            let host_environment = host.agent_environment()?;
            let system = match host_environment.get("LYREL_HOST_OS").map(String::as_str) {
                Some("windows") => env
                    .get("PROGRAMDATA")
                    .map(|path| host.join(Path::new(path), "opencode")),
                Some("macos") => Some(PathBuf::from("/Library/Application Support/opencode")),
                Some("linux") => Some(PathBuf::from("/etc/opencode")),
                _ => {
                    problems.push("OpenCode system skill source requires selected Host OS metadata; upgrade lyrel-server".into());
                    None
                }
            };
            if let Some(system) = system {
                push(
                    &mut roots,
                    host.join(&system, "skills"),
                    agent,
                    SourceKind::AgentPrivate,
                    true,
                    true,
                    DiscoveryState::Discovered,
                );
                configs.push(host.join(&system, "opencode.json"));
                configs.push(host.join(&system, "opencode.jsonc"));
            }
            for config in configs {
                let value = read_config(host, &config, problems);
                for value in strings(value.get("skills")) {
                    if value.starts_with("https://") || value.starts_with("http://") {
                        problems.push(format!("{}: online skill catalog not fetched by readonly disk browsing: {value}",config.display()));
                    } else {
                        add_path(
                            host,
                            &home,
                            request.project.as_deref(),
                            value,
                            agent,
                            &mut roots,
                            problems,
                        );
                    }
                }
            }
        }
        HookAgent::Hermes => {
            let dir = target.hermes_home()?;
            settings = read_config(host, &host.join(&dir, "config.yaml"), problems);
            let skill_config = settings.get("skills").cloned().unwrap_or(Value::Null);
            push(
                &mut roots,
                host.join(&dir, "skills"),
                agent,
                SourceKind::AgentPrivate,
                true,
                false,
                DiscoveryState::Discovered,
            );
            // Shared user skills are visible even when Hermes has not opted in.
            let shared = under(host, &home, &[".agents", "skills"]);
            let mut shared_enabled = false;
            for value in strings(skill_config.get("external_dirs"))
                .into_iter()
                .chain(strings(skill_config.get("create_dir")))
            {
                let path = resolve_path(host, &home, request.project.as_deref(), value)?;
                if path == shared {
                    shared_enabled = true;
                }
                push(
                    &mut roots,
                    path,
                    agent,
                    SourceKind::AgentPrivate,
                    true,
                    false,
                    DiscoveryState::Unknown,
                );
            }
            push(
                &mut roots,
                shared,
                agent,
                SourceKind::UserShared,
                true,
                false,
                if shared_enabled {
                    DiscoveryState::Discovered
                } else {
                    DiscoveryState::Disabled
                },
            );
            let project_root = project_dirs
                .last()
                .filter(|path| host.exists(&host.join(path, ".git")))
                .or(request.project.as_ref());
            if let Some(project) = project_root {
                for name in [".hermes", ".agents"] {
                    push(
                        &mut roots,
                        under(host, project, &[name, "skills"]),
                        agent,
                        if name == ".agents" {
                            SourceKind::ProjectShared
                        } else {
                            SourceKind::AgentPrivate
                        },
                        true,
                        false,
                        if bool_field(&skill_config, "project_discovery") == Some(false) {
                            DiscoveryState::Disabled
                        } else {
                            DiscoveryState::Unknown
                        },
                    );
                }
            }
        }
    }
    for root in &mut roots {
        if root.kind == SourceKind::ProjectShared {
            root.project = project_dirs
                .iter()
                .find(|project| under(host, project, &[".agents", "skills"]) == root.path)
                .cloned();
        }
    }
    if disabled && agent != HookAgent::Pi {
        for root in &mut roots {
            root.state = DiscoveryState::Disabled;
        }
    }
    // Parsing failures must not silently advertise an enabled native source.
    if !settings.is_null() && settings.get("permissions").is_some() {
        for root in &mut roots {
            if root.state != DiscoveryState::Disabled {
                root.state = DiscoveryState::Unknown;
            }
        }
    }
    Ok(roots)
}
fn push(
    roots: &mut Vec<Root>,
    path: PathBuf,
    agent: HookAgent,
    kind: SourceKind,
    recursive: bool,
    flat_markdown: bool,
    state: DiscoveryState,
) {
    roots.push(Root {
        path,
        agent,
        kind,
        recursive,
        flat_markdown,
        state,
        project: None,
    });
}
fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}
fn strings(value: Option<&Value>) -> Vec<&str> {
    match value {
        Some(Value::String(v)) => vec![v],
        Some(Value::Sequence(values)) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}
fn flag_values<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == flag {
            if let Some(value) = args.get(index + 1) {
                result.push(value.as_str());
                index += 1;
            }
        } else if let Some((key, value)) = args[index].split_once('=') {
            if key == flag {
                result.push(value);
            }
        }
        index += 1;
    }
    result
}
fn read_optional(host: &dyn Host, path: &Path, problems: &mut Vec<String>) -> Option<String> {
    match host.read_file(path, MAX_MARKDOWN) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(error) => {
                problems.push(format!("{}: {error}", path.display()));
                None
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            problems.push(format!("{}: {error}", path.display()));
            None
        }
    }
}
fn read_config(host: &dyn Host, path: &Path, problems: &mut Vec<String>) -> Value {
    let Some(text) = read_optional(host, path, problems) else {
        return Value::Null;
    };
    let yaml =
        path.to_string_lossy().ends_with(".yml") || path.to_string_lossy().ends_with(".yaml");
    let parsed = if yaml {
        yaml_serde::from_str(&text).map_err(|_| ())
    } else {
        jsonc(&text)
    };
    match parsed {
        Ok(value) => value,
        Err(()) => {
            problems.push(format!(
                "{}: cannot verify invalid native configuration",
                path.display()
            ));
            Value::Null
        }
    }
}
fn merge(target: &mut Value, source: Value) {
    if source.is_null() {
        return;
    }
    if let (Some(target), Some(source)) = (target.as_mapping_mut(), source.as_mapping()) {
        for (key, value) in source {
            if let Some(existing) = target.get_mut(key) {
                merge(existing, value.clone());
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = source;
    }
}
fn resolve_path(
    host: &dyn Host,
    home: &Path,
    cwd: Option<&Path>,
    value: &str,
) -> io::Result<PathBuf> {
    if value.contains("${") || value.contains('$') {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "skill path uses environment expansion not reported by selected Host",
        ));
    }
    if value.starts_with('~') || host.is_absolute(Path::new(value)) {
        absolute_selector(host, home, value)
    } else {
        cwd.map(|cwd| host.join(cwd, value)).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "relative skill path needs an explicit project on the selected Host",
            )
        })
    }
}
fn add_path(
    host: &dyn Host,
    home: &Path,
    cwd: Option<&Path>,
    value: &str,
    agent: HookAgent,
    roots: &mut Vec<Root>,
    problems: &mut Vec<String>,
) {
    match resolve_path(host, home, cwd, value) {
        Ok(path) => push(
            roots,
            path,
            agent,
            SourceKind::AgentPrivate,
            agent != HookAgent::OhMyPi,
            agent == HookAgent::Pi || agent == HookAgent::Kimi || agent == HookAgent::OpenCode,
            DiscoveryState::Unknown,
        ),
        Err(error) => problems.push(format!("{}: {value}: {error}", agent.display_name())),
    }
}
fn pi_paths(
    host: &dyn Host,
    home: &Path,
    dir: &Path,
    settings: &Value,
    agent: HookAgent,
    roots: &mut Vec<Root>,
    problems: &mut Vec<String>,
) {
    for value in strings(settings.get("skills")) {
        if value.starts_with(['!', '-']) {
            for root in roots.iter_mut() {
                root.state = DiscoveryState::Unknown;
            }
            continue;
        }
        add_path(
            host,
            home,
            Some(dir),
            value.trim_start_matches('+'),
            agent,
            roots,
            problems,
        );
    }
}
fn project_ancestors(
    host: &dyn Host,
    project: Option<&Path>,
    home: &Path,
) -> io::Result<Vec<PathBuf>> {
    let Some(project) = project else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    let mut current = project.to_path_buf();
    for _ in 0..64 {
        if current == home {
            break;
        }
        result.push(current.clone());
        match host.stat(&host.join(&current, ".git")) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let text = current.to_string_lossy();
        let trimmed = text.trim_end_matches(['/', '\\']);
        let Some(ix) = trimmed.rfind(['/', '\\']) else {
            break;
        };
        let parent = if ix == 0 {
            PathBuf::from(&trimmed[..1])
        } else {
            PathBuf::from(&trimmed[..ix])
        };
        if parent == current || !host.is_absolute(&parent) {
            break;
        }
        current = parent;
    }
    Ok(result)
}

fn jsonc(text: &str) -> Result<Value, ()> {
    let mut bytes = text.trim_start_matches('\u{feff}').as_bytes().to_vec();
    let mut index = 0;
    let mut string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            string = true;
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                bytes[index] = b' ';
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    bytes[index] = b' ';
                    bytes[index + 1] = b' ';
                    index += 2;
                    closed = true;
                    break;
                }
                if !matches!(bytes[index], b'\n' | b'\r') {
                    bytes[index] = b' ';
                }
                index += 1;
            }
            if !closed {
                return Err(());
            }
        } else {
            index += 1;
        }
    }
    index = 0;
    string = false;
    escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b',' {
            let next = bytes[index + 1..].iter().find(|b| !b.is_ascii_whitespace());
            if matches!(next, Some(b']' | b'}')) {
                bytes[index] = b' ';
            }
        }
        index += 1;
    }
    serde_json::from_slice(&bytes).map_err(|_| ())
}
