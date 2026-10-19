//! Readonly, Host-bound skill discovery. Installation identity is a canonical
//! directory on one Host, never a frontmatter name. No native runtime is queried
//! or reconfigured; a discovered file is not evidence of runtime activation.
#[cfg(feature = "github")]
pub mod catalog;
mod discovery;
#[cfg(feature = "github")]
pub mod install;
#[cfg(feature = "github")]
pub mod maintenance;
mod packages;
mod provenance;
use crate::{
    core::agent_hooks::{HookAgent, HookTarget},
    host::{Host, HostId, MTime},
};
use discovery::agent_roots;
pub use provenance::SkillOrigin;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_MARKDOWN: u64 = 1024 * 1024;
const MAX_DIRECTORIES: usize = 8_000;
const MAX_DEPTH: usize = 24;
const MAX_COMPANIONS: usize = 20_000;
const MAX_INSTALLATIONS: usize = 2_000;
const MAX_SNAPSHOT_MARKDOWN: u64 = 32 * 1024 * 1024;

struct DetailBudget {
    markdown: u64,
    companions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    UserShared,
    ProjectShared,
    AgentPrivate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryState {
    Discovered,
    Disabled,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct ScanRequest {
    home: PathBuf,
    project: Option<PathBuf>,
    working_directory: Option<PathBuf>,
    launch_commands: HashMap<String, String>,
}
impl ScanRequest {
    /// `home` must be the selected Host's reported home, never a GUI fallback.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            project: None,
            working_directory: None,
            launch_commands: HashMap::new(),
        }
    }
    pub fn with_project(mut self, project: impl Into<PathBuf>) -> Self {
        self.project = Some(project.into());
        self
    }
    pub fn with_launch_commands(mut self, commands: HashMap<String, String>) -> Self {
        self.launch_commands = commands;
        self
    }
    /// Explicit CLI working directory for relative XDG state selectors.
    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }
    pub fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }
    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillSource {
    agent: HookAgent,
    kind: SourceKind,
    entry: PathBuf,
    state: DiscoveryState,
}
impl SkillSource {
    pub fn agent(&self) -> HookAgent {
        self.agent
    }
    pub fn kind(&self) -> SourceKind {
        self.kind
    }
    /// Spelling encountered during discovery, including a link alias.
    pub fn entry(&self) -> &Path {
        &self.entry
    }
    pub fn state(&self) -> DiscoveryState {
        self.state
    }
}
#[derive(Clone, Debug, Default)]
pub struct Companions {
    files: u64,
    bytes: u64,
    incomplete: bool,
}
impl Companions {
    pub fn files(&self) -> u64 {
        self.files
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn is_incomplete(&self) -> bool {
        self.incomplete
    }
}
#[derive(Clone, Debug)]
pub struct SkillInstallation {
    host: HostId,
    document: PathBuf,
    path: PathBuf,
    name: String,
    description: Option<String>,
    invocation: Option<String>,
    origin: Option<String>,
    source_record: Option<SkillOrigin>,
    markdown: Option<String>,
    updated: Option<MTime>,
    sources: Vec<SkillSource>,
    companions: Companions,
    problems: Vec<String>,
}
impl SkillInstallation {
    pub fn host(&self) -> HostId {
        self.host
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn document(&self) -> &Path {
        &self.document
    }
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    pub fn invocation(&self) -> Option<&str> {
        self.invocation.as_deref()
    }
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }
    pub fn source_record(&self) -> Option<&SkillOrigin> {
        self.source_record.as_ref()
    }
    pub fn markdown(&self) -> Option<&str> {
        self.markdown.as_deref()
    }
    pub fn updated(&self) -> Option<MTime> {
        self.updated
    }
    pub fn sources(&self) -> &[SkillSource] {
        &self.sources
    }
    pub fn companions(&self) -> &Companions {
        &self.companions
    }
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}
#[derive(Clone, Debug)]
pub struct ScanSnapshot {
    host: HostId,
    project: Option<PathBuf>,
    scanned_at: SystemTime,
    installations: Vec<SkillInstallation>,
    problems: Vec<String>,
}
impl ScanSnapshot {
    pub fn host(&self) -> HostId {
        self.host
    }
    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }
    pub fn scanned_at(&self) -> SystemTime {
        self.scanned_at
    }
    pub fn installations(&self) -> &[SkillInstallation] {
        &self.installations
    }
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
    pub fn filtered<'a>(
        &'a self,
        query: &'a str,
        kind: Option<SourceKind>,
    ) -> impl Iterator<Item = &'a SkillInstallation> + 'a {
        let query = query.trim().to_lowercase();
        self.installations.iter().filter(move |skill| {
            kind.is_none_or(|kind| skill.sources.iter().any(|source| source.kind == kind))
                && (query.is_empty()
                    || skill.name.to_lowercase().contains(&query)
                    || skill
                        .description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                    || skill
                        .origin
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                    || skill.source_record.as_ref().is_some_and(|origin| {
                        origin
                            .source_url()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&query)
                            || origin
                                .skill_path()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(&query)
                    }))
        })
    }
}
struct Root {
    path: PathBuf,
    agent: HookAgent,
    kind: SourceKind,
    state: DiscoveryState,
    recursive: bool,
    flat_markdown: bool,
}

/// Blocking; callers dispatch through HostOps. An unsupported old peer fails
/// explicitly before scanning; a failed source remains a visible partial result.
pub fn scan(host: &dyn Host, request: &ScanRequest) -> io::Result<ScanSnapshot> {
    crate::host::guard_off_ui();
    if !host.is_connected() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "selected skill Host is disconnected",
        ));
    }
    host.agent_environment()?;
    if !host.is_absolute(&request.home)
        || request
            .project
            .as_deref()
            .is_some_and(|p| !host.is_absolute(p))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill home/project must be absolute on the selected Host",
        ));
    }
    if let Some(project) = &request.project {
        if !host.stat(project)?.is_dir {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "selected skill project is not a directory",
            ));
        }
    }
    let target = HookTarget::remote(host, request.home.clone())
        .with_launch_commands(&request.launch_commands);
    let mut snapshot = ScanSnapshot {
        host: host.id(),
        project: request.project.clone(),
        scanned_at: SystemTime::now(),
        installations: Vec::new(),
        problems: Vec::new(),
    };
    let mut roots = Vec::new();
    for agent in HookAgent::ALL {
        match agent_roots(host, &target, request, agent, &mut snapshot.problems) {
            Ok(found) => roots.extend(found),
            Err(error) => snapshot
                .problems
                .push(format!("{}: {error}", agent.display_name())),
        }
    }
    // A shared installation belongs to the selected Host home even when every
    // native launch profile overrides HOME. Browsable does not imply enabled.
    let shared = under(host, &request.home, &[".agents", "skills"]);
    for agent in HookAgent::ALL {
        if !roots.iter().any(|root| {
            root.agent == agent && root.kind == SourceKind::UserShared && root.path == shared
        }) {
            roots.push(Root {
                path: shared.clone(),
                agent,
                kind: SourceKind::UserShared,
                state: DiscoveryState::Unknown,
                recursive: true,
                flat_markdown: false,
            });
        }
    }
    let mut identities = HashMap::<PathBuf, usize>::new();
    let mut budget = MAX_DIRECTORIES;
    let mut detail_budget = DetailBudget {
        markdown: MAX_SNAPSHOT_MARKDOWN,
        companions: MAX_COMPANIONS,
    };
    for root in roots {
        let mut seen = HashSet::new();
        let mut pending = vec![(root.path.clone(), 0)];
        while let Some((directory, depth)) = pending.pop() {
            if budget == 0 || depth > MAX_DEPTH || snapshot.installations.len() >= MAX_INSTALLATIONS
            {
                snapshot.problems.push(format!(
                    "{}: skill directory scan limit reached",
                    directory.display()
                ));
                break;
            }
            budget -= 1;
            let canonical = match host.canonicalize(&directory) {
                Ok(path) => path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    snapshot
                        .problems
                        .push(format!("{}: {error}", directory.display()));
                    continue;
                }
            };
            if !seen.insert(canonical.clone()) {
                if let Some(&ix) = identities.get(&canonical) {
                    let source = SkillSource {
                        agent: root.agent,
                        kind: root.kind,
                        entry: directory.clone(),
                        state: root.state,
                    };
                    if !snapshot.installations[ix].sources.contains(&source) {
                        snapshot.installations[ix].sources.push(source);
                    }
                }
                continue;
            }
            match host.stat(&directory) {
                Ok(meta) if !meta.is_dir => {
                    let source = SkillSource {
                        agent: root.agent,
                        kind: root.kind,
                        entry: directory.clone(),
                        state: root.state,
                    };
                    if let Some(&ix) = identities.get(&canonical) {
                        if !snapshot.installations[ix].sources.contains(&source) {
                            snapshot.installations[ix].sources.push(source);
                        }
                    } else {
                        let skill = read_installation(
                            host,
                            canonical.clone(),
                            directory,
                            source,
                            &mut detail_budget,
                        );
                        identities.insert(canonical, snapshot.installations.len());
                        snapshot.installations.push(skill);
                    }
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    snapshot
                        .problems
                        .push(format!("{}: {error}", directory.display()));
                    continue;
                }
            }
            let file = host.join(&directory, "SKILL.md");
            match host.stat(&file) {
                Ok(meta) if !meta.is_dir => {
                    let source = SkillSource {
                        agent: root.agent,
                        kind: root.kind,
                        entry: directory.clone(),
                        state: root.state,
                    };
                    if let Some(&ix) = identities.get(&canonical) {
                        if !snapshot.installations[ix].sources.contains(&source) {
                            snapshot.installations[ix].sources.push(source);
                        }
                    } else {
                        let skill = read_installation(
                            host,
                            canonical.clone(),
                            file,
                            source,
                            &mut detail_budget,
                        );
                        identities.insert(canonical, snapshot.installations.len());
                        snapshot.installations.push(skill);
                    }
                    // A skill's resources are not additional installed skills.
                    continue;
                }
                Ok(_) => snapshot
                    .problems
                    .push(format!("{}: SKILL.md is a directory", file.display())),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => snapshot
                    .problems
                    .push(format!("{}: {error}", file.display())),
            }
            if !root.recursive && depth >= 1 {
                continue;
            }
            match host.read_dir(&directory, None) {
                Ok(entries) => {
                    for entry in entries
                        .into_iter()
                        .rev()
                        .filter(|e| !e.name.starts_with('.') && e.name != "node_modules")
                    {
                        if snapshot.installations.len() >= MAX_INSTALLATIONS {
                            snapshot.problems.push(format!(
                                "{}: skill installation scan limit reached",
                                directory.display()
                            ));
                            break;
                        }
                        let path = host.join(&directory, &entry.name);
                        if entry.is_dir {
                            pending.push((path, depth + 1));
                        } else if depth == 0 && root.flat_markdown && entry.name.ends_with(".md") {
                            match host.canonicalize(&path) {
                                Ok(canonical) => {
                                    let source = SkillSource {
                                        agent: root.agent,
                                        kind: root.kind,
                                        entry: path.clone(),
                                        state: root.state,
                                    };
                                    if let Some(&ix) = identities.get(&canonical) {
                                        if !snapshot.installations[ix].sources.contains(&source) {
                                            snapshot.installations[ix].sources.push(source);
                                        }
                                    } else {
                                        let skill = read_installation(
                                            host,
                                            canonical.clone(),
                                            path,
                                            source,
                                            &mut detail_budget,
                                        );
                                        identities.insert(canonical, snapshot.installations.len());
                                        snapshot.installations.push(skill);
                                    }
                                }
                                Err(error) => snapshot
                                    .problems
                                    .push(format!("{}: {error}", path.display())),
                            }
                        }
                    }
                }
                Err(error) => snapshot
                    .problems
                    .push(format!("{}: {error}", directory.display())),
            }
        }
    }
    provenance::attach(host, &target, request, &mut snapshot);
    snapshot
        .installations
        .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    Ok(snapshot)
}

fn absolute_selector(host: &dyn Host, home: &Path, value: &str) -> io::Result<PathBuf> {
    let path = if value == "~" {
        home.to_path_buf()
    } else if value.starts_with("~/") || value.starts_with("~\\") {
        host.join(home, &value[2..])
    } else {
        PathBuf::from(value)
    };
    if host.is_absolute(&path) {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "relative native scope needs its executing Host working directory; not falling back to desktop",
        ))
    }
}
fn under(host: &dyn Host, root: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(root.to_path_buf(), |path, part| host.join(&path, part))
}

fn read_installation(
    host: &dyn Host,
    path: PathBuf,
    file: PathBuf,
    source: SkillSource,
    budget: &mut DetailBudget,
) -> SkillInstallation {
    let fallback = path
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("SKILL.md")
        .to_string();
    let mut skill = SkillInstallation {
        host: host.id(),
        document: host.canonicalize(&file).unwrap_or_else(|_| file.clone()),
        path,
        name: fallback,
        description: None,
        invocation: None,
        origin: None,
        source_record: None,
        markdown: None,
        updated: None,
        sources: vec![source],
        companions: Companions::default(),
        problems: Vec::new(),
    };
    match host.stat(&file) {
        Ok(meta) => skill.updated = meta.mtime,
        Err(error) => skill.problems.push(format!("{}: {error}", file.display())),
    }
    let limit = MAX_MARKDOWN.min(budget.markdown);
    match host.read_file(&file, limit) {
        Ok(bytes) => {
            budget.markdown = budget.markdown.saturating_sub(bytes.len() as u64);
            match String::from_utf8(bytes) {
                Ok(markdown) => {
                    let frontmatter = frontmatter(&markdown);
                    if let Some(head) = frontmatter {
                        match yaml_serde::from_str::<BTreeMap<String, yaml_serde::Value>>(&head) {
                            Ok(fields) => {
                                skill.description = fields
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_owned);
                                if let Some(name) = fields
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .filter(|v| !v.trim().is_empty())
                                {
                                    skill.name = name.to_owned();
                                } else {
                                    skill
                                        .problems
                                        .push("SKILL.md: missing or invalid name".into());
                                }
                                if skill
                                    .description
                                    .as_ref()
                                    .is_none_or(|v| v.trim().is_empty())
                                {
                                    skill
                                        .problems
                                        .push("SKILL.md: missing or invalid description".into());
                                }
                                skill.invocation = fields
                                    .get("argument-hint")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_owned);
                                skill.origin = fields
                                    .get("metadata")
                                    .and_then(|v| v.get("source"))
                                    .and_then(|v| v.as_str())
                                    .map(str::to_owned);
                            }
                            Err(error) => skill.problems.push(format!("SKILL.md: {error}")),
                        }
                    } else if skill.document == skill.path
                        && skill
                            .sources
                            .iter()
                            .any(|source| source.agent == HookAgent::Kimi)
                    {
                        skill.name = skill
                            .path
                            .file_stem()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| skill.name.clone());
                        skill.description = markdown
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .map(|line| line.chars().take(240).collect());
                    } else {
                        skill
                            .problems
                            .push("SKILL.md: missing or unterminated YAML frontmatter".into());
                    }
                    if markdown.trim().is_empty() {
                        skill.problems.push("SKILL.md: empty content".into());
                    }
                    skill.markdown = Some(markdown);
                }
                Err(error) => skill
                    .problems
                    .push(format!("SKILL.md: invalid UTF-8: {error}")),
            }
        }
        Err(error) => skill.problems.push(format!(
            "SKILL.md: unreadable or exceeds remaining scan limit ({limit} bytes): {error}"
        )),
    }
    if skill.document != skill.path {
        skill.companions = companions(host, &skill.path, &mut budget.companions);
    }
    skill
}
fn companions(host: &dyn Host, root: &Path, remaining: &mut usize) -> Companions {
    let mut result = Companions::default();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((directory, depth)) = pending.pop() {
        let entries = match host.read_dir(&directory, None) {
            Ok(entries) => entries,
            Err(_) => {
                result.incomplete = true;
                continue;
            }
        };
        for entry in entries {
            if *remaining == 0 {
                result.incomplete = true;
                return result;
            }
            *remaining -= 1;
            if entry.is_symlink {
                result.incomplete = true;
                continue;
            }
            let path = host.join(&directory, &entry.name);
            if entry.is_dir {
                if depth < MAX_DEPTH && entry.name != ".git" {
                    pending.push((path, depth + 1));
                } else {
                    result.incomplete = true;
                }
            } else if directory != root || entry.name != "SKILL.md" {
                match host.stat(&path) {
                    Ok(meta) => {
                        result.files += 1;
                        result.bytes = result.bytes.saturating_add(meta.len);
                    }
                    Err(_) => result.incomplete = true,
                }
            }
        }
    }
    result
}

fn frontmatter(markdown: &str) -> Option<String> {
    let mut lines = markdown.trim_start_matches('\u{feff}').lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut header = String::new();
    for line in lines {
        if line == "---" {
            return Some(header);
        }
        if header.len() + line.len() > 64 * 1024 {
            return None;
        }
        header.push_str(line);
        header.push('\n');
    }
    None
}
