//! Readonly skills CLI 1.7.2 source records. Match the canonical shared
//! installation path, not just its frontmatter name. Unknown locks never write.
use super::*;
#[derive(Clone, Debug)]
pub struct SkillOrigin {
    source: String,
    source_type: String,
    source_url: Option<String>,
    reference: Option<String>,
    skill_path: Option<String>,
    lock_path: PathBuf,
}
impl SkillOrigin {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn source_type(&self) -> &str {
        &self.source_type
    }
    pub fn source_url(&self) -> Option<&str> {
        self.source_url.as_deref()
    }
    pub fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }
    pub fn skill_path(&self) -> Option<&str> {
        self.skill_path.as_deref()
    }
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}
pub(super) fn attach(
    host: &dyn Host,
    target: &HookTarget<'_>,
    request: &ScanRequest,
    snapshot: &mut ScanSnapshot,
) {
    let mut locks = Vec::new();
    let mut seen = HashSet::new();
    for agent in HookAgent::ALL {
        if let Ok(environment) = target.agent_environment(agent) {
            if let Ok(home) = target.agent_home(&environment) {
                let lock = if let Some(state) = environment
                    .get("XDG_STATE_HOME")
                    .filter(|value| !value.is_empty())
                {
                    let state = match absolute_selector(host, &home, state) {
                        Ok(path) => path,
                        Err(error) => {
                            snapshot
                                .problems
                                .push(format!("skills CLI source lock: {error}"));
                            continue;
                        }
                    };
                    under(host, &state, &["skills", ".skill-lock.json"])
                } else {
                    under(host, &home, &[".agents", ".skill-lock.json"])
                };
                locks.push((lock, under(host, &home, &[".agents", "skills"]), 3));
            }
        }
    }
    if let Some(project) = &request.project {
        locks.push((
            host.join(project, "skills-lock.json"),
            under(host, project, &[".agents", "skills"]),
            1,
        ));
    }
    for (lock, root, version) in locks {
        if !seen.insert(lock.clone()) {
            continue;
        }
        let bytes = match host.read_file(&lock, MAX_MARKDOWN) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                snapshot
                    .problems
                    .push(format!("{}: {error}", lock.display()));
                continue;
            }
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            snapshot.problems.push(format!(
                "{}: invalid skills CLI source record; no source inferred",
                lock.display()
            ));
            continue;
        };
        if value.get("version").and_then(|v| v.as_u64()) != Some(version) {
            snapshot.problems.push(format!(
                "{}: unsupported skills CLI source record version",
                lock.display()
            ));
            continue;
        }
        let Some(entries) = value.get("skills").and_then(|v| v.as_object()) else {
            snapshot
                .problems
                .push(format!("{}: invalid skills CLI skills map", lock.display()));
            continue;
        };
        for (name, entry) in entries {
            let directory = sanitize_name(name);
            let Ok(installation) = host.canonicalize(&host.join(&root, &directory)) else {
                continue;
            };
            let Some(skill) = snapshot
                .installations
                .iter_mut()
                .find(|skill| skill.path == installation)
            else {
                continue;
            };
            let (Some(source), Some(source_type)) = (
                entry.get("source").and_then(|v| v.as_str()),
                entry.get("sourceType").and_then(|v| v.as_str()),
            ) else {
                skill
                    .problems
                    .push(format!("{}: incomplete source record", lock.display()));
                continue;
            };
            skill.origin = Some(source.to_string());
            skill.source_record = Some(SkillOrigin {
                source: source.into(),
                source_type: source_type.into(),
                source_url: entry
                    .get("sourceUrl")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                reference: entry.get("ref").and_then(|v| v.as_str()).map(str::to_owned),
                skill_path: entry
                    .get("skillPath")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                lock_path: lock.clone(),
            });
        }
    }
}
fn sanitize_name(name: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '_' {
            result.push(ch);
            separator = false;
        } else if !separator {
            result.push('-');
            separator = true;
        }
    }
    let result = result.trim_matches(['.', '-']);
    let result = &result[..result.len().min(255)];
    if result.is_empty() {
        "unnamed-skill".into()
    } else {
        result.into()
    }
}
