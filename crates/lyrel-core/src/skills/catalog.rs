//! Anonymous public GitHub skill catalogs and the skills.sh CLI search endpoint.
//! Network I/O is injected through the existing GitHub Transport boundary.

use crate::core::github::{ApiError, Transport};
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    Source(String),
    Unsafe(String),
    Invalid(String),
    Limit(String),
    Http(ApiError),
}
impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(message)
            | Self::Unsafe(message)
            | Self::Invalid(message)
            | Self::Limit(message) => f.write_str(message),
            Self::Http(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CatalogError {}
impl From<ApiError> for CatalogError {
    fn from(error: ApiError) -> Self {
        Self::Http(error)
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubSource {
    pub owner: String,
    pub repo: String,
    pub reference: Option<String>,
    pub subpath: Option<String>,
    pub skill_filter: Option<String>,
}
impl GitHubSource {
    pub fn parse(input: &str) -> Result<Self, CatalogError> {
        let input = input.trim();
        let (body, fragment) = input
            .split_once('#')
            .map_or((input, None), |(body, fragment)| (body, Some(fragment)));
        let mut reference = None;
        let mut skill_filter = None;
        if let Some(fragment) = fragment {
            let (value, skill) = fragment
                .split_once('@')
                .map_or((fragment, None), |(value, skill)| (value, Some(skill)));
            reference = Some(decode(value)?);
            if let Some(skill) = skill {
                skill_filter = Some(decode(skill)?);
            }
        }
        let (path, is_url) = if body.starts_with("https://") {
            let url = url::Url::parse(body)
                .map_err(|_| CatalogError::Source("Invalid GitHub URL".into()))?;
            if url.host_str() != Some("github.com")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
            {
                return Err(CatalogError::Source(
                    "Only anonymous github.com HTTPS sources are supported".into(),
                ));
            }
            // Inspect the original path: URL normalization must not erase traversal.
            (
                body.strip_prefix("https://github.com/")
                    .ok_or_else(|| CatalogError::Source("Invalid GitHub URL".into()))?
                    .to_string(),
                true,
            )
        } else {
            let path = body.strip_prefix("github:").unwrap_or(body);
            if path.contains(':') || path.starts_with(['.', '/', '\\']) || path.contains("//") {
                return Err(CatalogError::Source(
                    "Only public GitHub repositories are supported".into(),
                ));
            }
            (path.to_string(), false)
        };
        let (path, suffix) = path
            .rsplit_once('@')
            .map_or((path.as_str(), None), |(path, skill)| (path, Some(skill)));
        if let Some(skill) = suffix {
            if skill_filter.is_some() {
                return Err(CatalogError::Source("Duplicate skill selection".into()));
            }
            skill_filter = Some(decode(skill)?);
        }
        let parts = path.split('/').map(decode).collect::<Result<Vec<_>, _>>()?;
        if parts.len() < 2 {
            return Err(CatalogError::Source("Expected owner/repository".into()));
        }
        let owner = parts[0].clone();
        let repo = parts[1]
            .strip_suffix(".git")
            .unwrap_or(&parts[1])
            .to_string();
        if !valid_repo_part(&owner) || !valid_repo_part(&repo) {
            return Err(CatalogError::Source(
                "Invalid GitHub owner or repository".into(),
            ));
        }
        let tail = &parts[2..];
        let subpath = if is_url && !tail.is_empty() {
            if tail.len() < 2 || tail[0] != "tree" {
                return Err(CatalogError::Source(
                    "Only GitHub repository and tree URLs are supported".into(),
                ));
            }
            if reference.is_none() {
                reference = Some(tail[1].clone());
            }
            if tail.len() > 2 {
                Some(tail[2..].join("/"))
            } else {
                None
            }
        } else if !tail.is_empty() {
            Some(tail.join("/"))
        } else {
            None
        };
        if let Some(path) = &subpath {
            safe_path(path)?;
        }
        if let Some(value) = &reference {
            validate_reference(value)?;
        }
        if skill_filter.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control)
        }) {
            return Err(CatalogError::Source("Invalid skill selection".into()));
        }
        Ok(Self {
            owner,
            repo,
            reference,
            subpath,
            skill_filter,
        })
    }
    /// Overrides are literal UI values, not URL fragments. `None` preserves the
    /// value from the source URL, including a skills.sh skill filter.
    pub fn with_selection(
        mut self,
        reference: Option<&str>,
        subpath: Option<&str>,
    ) -> Result<Self, CatalogError> {
        if let Some(reference) = reference {
            validate_reference(reference)?;
            self.reference = Some(reference.to_string());
        }
        if let Some(subpath) = subpath {
            safe_path(subpath)?;
            self.subpath = Some(subpath.to_string());
        }
        Ok(self)
    }
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.repo)
    }
}
fn validate_reference(value: &str) -> Result<(), CatalogError> {
    if value.is_empty()
        || value.len() > 255
        || value == "@"
        || value.ends_with('.')
        || value.contains("..")
        || value.contains("@{")
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "\\~^:?*[".contains(c))
        || value
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(CatalogError::Source("Invalid GitHub ref".into()));
    }
    Ok(())
}
fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn decode(value: &str) -> Result<String, CatalogError> {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err(CatalogError::Source("Invalid percent escape".into()));
        }
    }
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| CatalogError::Source("Source is not UTF-8".into()))
}
fn safe_path(path: &str) -> Result<(), CatalogError> {
    if path.is_empty()
        || path.len() > 1024
        || path.split('/').any(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            part.is_empty()
                || part.encode_utf16().count() > 255
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
                || matches!(
                    stem.as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
    {
        return Err(CatalogError::Unsafe(format!(
            "Unsafe repository path: {path}"
        )));
    }
    Ok(())
}

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_TREE_ENTRIES: usize = 20_000;
const MAX_FILES: usize = 1_000;
const MAX_TOTAL: u64 = 50 * 1024 * 1024;
const MAX_SKILL: u64 = 1024 * 1024;

#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct SkillCandidate {
    pub name: String,
    pub description: String,
    pub source: GitHubSource,
    pub skill_path: String,
    pub tree_hash: String,
}
impl SkillCandidate {
    pub fn new(
        name: String,
        description: String,
        source: GitHubSource,
        skill_path: String,
        tree_hash: String,
    ) -> Self {
        Self {
            name,
            description,
            source,
            skill_path,
            tree_hash,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn description(&self) -> &str {
        &self.description
    }
    pub fn source(&self) -> &GitHubSource {
        &self.source
    }
    pub fn skill_path(&self) -> &str {
        &self.skill_path
    }
    pub fn tree_hash(&self) -> &str {
        &self.tree_hash
    }
}
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct SkillFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}
impl SkillFile {
    pub fn new(path: PathBuf, contents: Vec<u8>) -> Self {
        Self { path, contents }
    }
}
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct PreparedSkill {
    pub candidate: SkillCandidate,
    pub files: Vec<SkillFile>,
    pub computed_hash: String,
}
impl PreparedSkill {
    pub fn new(candidate: SkillCandidate, files: Vec<SkillFile>, computed_hash: String) -> Self {
        Self {
            candidate,
            files,
            computed_hash,
        }
    }
}
#[derive(Clone, Debug)]
pub struct SkillCatalog {
    source: GitHubSource,
    tree_sha: String,
    entries: Vec<TreeEntry>,
    candidates: Vec<SkillCandidate>,
    documents: BTreeMap<String, Vec<u8>>,
    problems: Vec<String>,
}
impl SkillCatalog {
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
    pub fn candidates(&self) -> &[SkillCandidate] {
        &self.candidates
    }
    /// Immutable Git tree selected when discovery completed, not a moving ref.
    pub fn tree_sha(&self) -> &str {
        &self.tree_sha
    }
    /// Applies the same bounded download budget to the whole user selection.
    pub fn prepare_many(
        &self,
        http: &dyn Transport,
        indices: &[usize],
    ) -> Result<Vec<PreparedSkill>, CatalogError> {
        anonymous(http)?;
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for index in indices {
            if !seen.insert(*index) {
                return Err(CatalogError::Invalid("Duplicate selected skill".into()));
            }
            let candidate = self.candidates.get(*index).ok_or_else(|| {
                CatalogError::Invalid("Select an existing skill candidate".into())
            })?;
            let folder = candidate.skill_path.strip_suffix("SKILL.md").unwrap_or("");
            for entry in self.entries.iter().filter(|entry| {
                entry.kind == "blob"
                    && entry.path.starts_with(folder)
                    && !excluded(&entry.path[folder.len()..])
            }) {
                selected.push(entry);
                if selected.len() > MAX_FILES {
                    return Err(CatalogError::Limit("Selection exceeds 1,000 files".into()));
                }
            }
        }
        check_budget(&selected)?;
        indices
            .iter()
            .map(|index| self.prepare(http, *index))
            .collect()
    }
    pub fn prepare(
        &self,
        http: &dyn Transport,
        index: usize,
    ) -> Result<PreparedSkill, CatalogError> {
        anonymous(http)?;
        let candidate = self
            .candidates
            .get(index)
            .ok_or_else(|| CatalogError::Invalid("Select an existing skill candidate".into()))?;
        let folder = candidate.skill_path.strip_suffix("SKILL.md").unwrap_or("");
        let selected = self
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == "blob"
                    && entry.path.starts_with(folder)
                    && !excluded(&entry.path[folder.len()..])
            })
            .collect::<Vec<_>>();
        check_budget(&selected)?;
        let mut files = Vec::with_capacity(selected.len());
        for entry in selected {
            let contents = if let Some(document) = self.documents.get(&entry.sha) {
                document.clone()
            } else {
                fetch_blob(http, &self.source, entry, MAX_TOTAL)?
            };
            files.push(SkillFile {
                path: PathBuf::from(&entry.path[folder.len()..]),
                contents,
            });
        }
        let computed_hash = computed_hash(&files)?;
        Ok(PreparedSkill {
            candidate: candidate.clone(),
            files,
            computed_hash,
        })
    }
}
#[derive(Clone, Debug, Deserialize)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    size: Option<u64>,
}
#[derive(Deserialize)]
struct TreeReply {
    sha: String,
    truncated: bool,
    tree: Vec<TreeEntry>,
}
#[derive(Deserialize)]
struct BlobReply {
    sha: String,
    size: u64,
    encoding: String,
    content: String,
}
fn anonymous(http: &dyn Transport) -> Result<(), CatalogError> {
    if http.authenticated() {
        return Err(CatalogError::Unsafe(
            "Skill discovery only accepts anonymous public HTTP".into(),
        ));
    }
    Ok(())
}
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}
fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn excluded(path: &str) -> bool {
    path.split('/')
        .any(|part| matches!(part, ".git" | "node_modules"))
}
fn check_budget(entries: &[&TreeEntry]) -> Result<(), CatalogError> {
    if entries.len() > MAX_FILES {
        return Err(CatalogError::Limit("Skill exceeds 1,000 files".into()));
    }
    let mut bytes = 0u64;
    for entry in entries {
        let size = entry
            .size
            .ok_or_else(|| CatalogError::Invalid("Blob size is missing".into()))?;
        if entry.path.rsplit('/').next() == Some("SKILL.md") && size > MAX_SKILL {
            return Err(CatalogError::Limit("SKILL.md exceeds 1 MiB".into()));
        }
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| CatalogError::Limit("Skill size overflow".into()))?;
        if bytes > MAX_TOTAL {
            return Err(CatalogError::Limit("Skill exceeds 50 MiB".into()));
        }
    }
    Ok(())
}
fn fetch_blob(
    http: &dyn Transport,
    source: &GitHubSource,
    entry: &TreeEntry,
    limit: u64,
) -> Result<Vec<u8>, CatalogError> {
    let size = entry
        .size
        .ok_or_else(|| CatalogError::Invalid("Missing blob size".into()))?;
    if size > limit {
        return Err(CatalogError::Limit(format!(
            "Blob exceeds download limit: {}",
            entry.path
        )));
    }
    let reply = http.get(&format!(
        "/repos/{}/git/blobs/{}",
        source.repository(),
        entry.sha
    ))?;
    // Reject before deserializing a response whose base64 payload cannot fit.
    if reply.body.len() as u64 > limit.saturating_mul(2).saturating_add(4096) {
        return Err(CatalogError::Limit(
            "Blob response exceeds download limit".into(),
        ));
    }
    let blob: BlobReply = serde_json::from_slice(&reply.body)
        .map_err(|e| CatalogError::Invalid(format!("Invalid GitHub blob: {e}")))?;
    if blob.sha != entry.sha || blob.size != size || blob.encoding != "base64" {
        return Err(CatalogError::Invalid(
            "GitHub blob does not match the selected tree".into(),
        ));
    }
    let content = blob
        .content
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if content.len() as u64 > size.div_ceil(3) * 4 {
        return Err(CatalogError::Limit(
            "Blob encoded size exceeds its declaration".into(),
        ));
    }
    let contents = STANDARD
        .decode(content)
        .map_err(|_| CatalogError::Invalid("Invalid GitHub blob base64".into()))?;
    if contents.len() as u64 != size {
        return Err(CatalogError::Invalid(
            "GitHub blob size does not match its declaration".into(),
        ));
    }
    Ok(contents)
}

/// Discover without writing anything. Every entry is validated before any blob
/// fetch; truncated responses and links never become partial trusted catalogs.
pub fn discover(http: &dyn Transport, source: &GitHubSource) -> Result<SkillCatalog, CatalogError> {
    anonymous(http)?;
    // Public struct fields are convenient for consumers, but never a bypass.
    if !valid_repo_part(&source.owner) || !valid_repo_part(&source.repo) {
        return Err(CatalogError::Source("Invalid repository source".into()));
    }
    if let Some(path) = &source.subpath {
        safe_path(path)?;
    }
    if let Some(reference) = &source.reference {
        GitHubSource::parse(&format!("{}#{}", source.repository(), encoded(reference)))?;
    }
    if source.skill_filter.as_ref().is_some_and(|filter| {
        filter.is_empty() || filter.len() > 255 || filter.chars().any(char::is_control)
    }) {
        return Err(CatalogError::Source("Invalid skill selection".into()));
    }
    let reference = encoded(source.reference.as_deref().unwrap_or("HEAD"));
    let reply = http.get(&format!(
        "/repos/{}/git/trees/{reference}?recursive=1",
        source.repository()
    ))?;
    if reply.body.len() > 16 * 1024 * 1024 {
        return Err(CatalogError::Limit(
            "GitHub tree response exceeds 16 MiB".into(),
        ));
    }
    let tree: TreeReply = serde_json::from_slice(&reply.body)
        .map_err(|e| CatalogError::Invalid(format!("Invalid GitHub tree: {e}")))?;
    if tree.truncated {
        return Err(CatalogError::Limit(
            "GitHub returned a truncated repository tree".into(),
        ));
    }
    if !valid_sha(&tree.sha) {
        return Err(CatalogError::Invalid("Invalid GitHub tree SHA".into()));
    }
    if tree.tree.len() > MAX_TREE_ENTRIES {
        return Err(CatalogError::Limit(
            "Repository tree exceeds 20,000 entries".into(),
        ));
    }
    let mut seen = HashSet::with_capacity(tree.tree.len());
    let mut kinds = BTreeMap::new();
    for entry in &tree.tree {
        safe_path(&entry.path)?;
        if !valid_sha(&entry.sha)
            || !matches!(
                (entry.kind.as_str(), entry.mode.as_str()),
                ("tree", "040000") | ("blob", "100644" | "100755")
            )
        {
            return Err(CatalogError::Unsafe(format!(
                "Unsupported repository entry (links and submodules are forbidden): {}",
                entry.path
            )));
        }
        if !seen.insert(entry.path.to_lowercase()) {
            return Err(CatalogError::Unsafe(format!(
                "Duplicate or case-colliding repository path: {}",
                entry.path
            )));
        }
        kinds.insert(entry.path.as_str(), entry.kind.as_str());
    }
    for entry in &tree.tree {
        if let Some((parent, _)) = entry.path.rsplit_once('/') {
            if kinds.get(parent) != Some(&"tree") {
                return Err(CatalogError::Unsafe(format!(
                    "Missing or non-directory repository parent: {parent}"
                )));
            }
        }
    }
    let subprefix = source.subpath.as_ref().map(|p| format!("{p}/"));
    let documents = tree
        .tree
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.kind == "blob"
                && entry.path.rsplit('/').next() == Some("SKILL.md")
                && !excluded(&entry.path)
                && subprefix
                    .as_ref()
                    .is_none_or(|prefix| entry.path.starts_with(prefix))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    check_budget(
        &documents
            .iter()
            .map(|index| &tree.tree[*index])
            .collect::<Vec<_>>(),
    )?;
    let mut catalog = SkillCatalog {
        source: source.clone(),
        tree_sha: tree.sha,
        entries: tree.tree,
        candidates: Vec::new(),
        documents: BTreeMap::new(),
        problems: Vec::new(),
    };
    for index in documents {
        let entry = &catalog.entries[index];
        let folder = entry.path.strip_suffix("SKILL.md").unwrap_or("");
        let contents = fetch_blob(http, source, entry, MAX_SKILL)?;
        let (name, description) = match skill_metadata(&contents, &entry.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                catalog.problems.push(error.to_string());
                continue;
            }
        };
        if source
            .skill_filter
            .as_ref()
            .is_some_and(|filter| filter != &name && slug(filter) != slug(&name))
        {
            continue;
        }
        let tree_hash = if folder.is_empty() {
            catalog.tree_sha.clone()
        } else {
            catalog
                .entries
                .iter()
                .find(|e| e.path == folder.trim_end_matches('/') && e.kind == "tree")
                .map(|e| e.sha.clone())
                .ok_or_else(|| CatalogError::Invalid("Missing skill directory tree SHA".into()))?
        };
        catalog.documents.insert(entry.sha.clone(), contents);
        catalog.candidates.push(SkillCandidate {
            name,
            description,
            source: source.clone(),
            skill_path: entry.path.clone(),
            tree_hash,
        });
    }
    catalog
        .candidates
        .sort_by(|a, b| a.skill_path.cmp(&b.skill_path));
    Ok(catalog)
}
fn skill_metadata(contents: &[u8], path: &str) -> Result<(String, String), CatalogError> {
    let text = std::str::from_utf8(contents)
        .map_err(|_| CatalogError::Invalid(format!("SKILL.md is not UTF-8: {path}")))?;
    let header = super::frontmatter(text)
        .ok_or_else(|| CatalogError::Invalid(format!("Missing YAML skill frontmatter: {path}")))?;
    let fields: BTreeMap<String, yaml_serde::Value> = yaml_serde::from_str(&header)
        .map_err(|e| CatalogError::Invalid(format!("Invalid skill frontmatter at {path}: {e}")))?;
    let field = |key: &str| -> Result<String, CatalogError> {
        fields
            .get(key)
            .and_then(yaml_serde::Value::as_str)
            .filter(|s| {
                !s.trim().is_empty() && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
            })
            .map(str::to_string)
            .ok_or_else(|| CatalogError::Invalid(format!("Skill requires string {key}: {path}")))
    };
    Ok((field("name")?, field("description")?))
}
fn slug(value: &str) -> String {
    value
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// skills CLI 1.7.2 computes SHA-256(path + bytes), ordered with ICU collation
/// (`localeCompare`), not Rust's byte ordering. No filesystem is read here.
pub fn computed_hash(files: &[SkillFile]) -> Result<String, CatalogError> {
    static COLLATOR: std::sync::LazyLock<Result<icu_collator::CollatorBorrowed<'static>, String>> =
        std::sync::LazyLock::new(|| {
            icu_collator::Collator::try_new(Default::default(), Default::default())
                .map_err(|e| e.to_string())
        });
    let collator = COLLATOR
        .as_ref()
        .map_err(|e| CatalogError::Invalid(e.clone()))?;
    let mut ordered = Vec::with_capacity(files.len());
    for file in files {
        let path = file
            .path
            .to_str()
            .ok_or_else(|| CatalogError::Unsafe("Skill path is not UTF-8".into()))?;
        let path = if path.contains('\\') {
            std::borrow::Cow::Owned(path.replace('\\', "/"))
        } else {
            std::borrow::Cow::Borrowed(path)
        };
        safe_path(&path)?;
        if !excluded(&path) {
            ordered.push((path, file.contents.as_slice()));
        }
    }
    ordered.sort_by(|a, b| collator.compare(&a.0, &b.0));
    let mut digest = Sha256::new();
    for (path, bytes) in ordered {
        digest.update(path.as_bytes());
        digest.update(bytes);
    }
    Ok(crate::daemon::install::checksums::hex(
        &digest.finalize().into(),
    ))
}

#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct SkillSearchResult {
    pub id: String,
    pub name: String,
    pub source: GitHubSource,
    pub skill_id: String,
    pub installs: u64,
}
#[derive(Deserialize)]
struct SearchReply {
    skills: Vec<SearchEntry>,
}
#[derive(Deserialize)]
struct SearchEntry {
    id: String,
    name: String,
    source: String,
    #[serde(rename = "skillId")]
    skill_id: Option<String>,
    #[serde(default)]
    installs: u64,
}
pub fn search(http: &dyn Transport, query: &str) -> Result<Vec<SkillSearchResult>, CatalogError> {
    anonymous(http)?;
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query.len() > 256 || query.chars().any(char::is_control) {
        return Err(CatalogError::Source(
            "Search query is invalid or too long".into(),
        ));
    }
    let reply = http.get(&format!("/api/search?q={}&limit=20", encoded(query)))?;
    if reply.body.len() > 1024 * 1024 {
        return Err(CatalogError::Limit("Search response exceeds 1 MiB".into()));
    }
    let decoded: SearchReply = serde_json::from_slice(&reply.body)
        .map_err(|e| CatalogError::Invalid(format!("Invalid skills.sh search response: {e}")))?;
    if decoded.skills.len() > 100 {
        return Err(CatalogError::Limit("Too many search results".into()));
    }
    let mut rows = Vec::with_capacity(decoded.skills.len());
    for item in decoded.skills {
        // The public catalogue also contains well-known HTTP registries, which
        // are outside this installer. They must not hide valid GitHub matches.
        let Ok(mut source) = GitHubSource::parse(&item.source) else {
            continue;
        };
        if source.subpath.is_some() || source.reference.is_some() || source.skill_filter.is_some() {
            continue;
        }
        let skill_id = item
            .skill_id
            .unwrap_or_else(|| item.id.rsplit('/').next().unwrap_or("").to_string());
        if skill_id.is_empty() || skill_id.len() > 255 || skill_id.chars().any(char::is_control) {
            return Err(CatalogError::Invalid(
                "Search result has an invalid skill id".into(),
            ));
        }
        source.skill_filter = Some(skill_id.clone());
        rows.push(SkillSearchResult {
            id: clean_metadata(&item.id),
            name: clean_metadata(&item.name),
            source,
            skill_id,
            installs: item.installs,
        });
    }
    rows.sort_by(|a, b| b.installs.cmp(&a.installs));
    Ok(rows)
}
fn clean_metadata(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(512)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A separate anonymous Transport for the CLI's undocumented skills.sh endpoint.
/// Deliberately has no token constructor, credential lookup, or API-v1 route.
pub struct SkillsSearchHttp {
    agent: ureq::Agent,
}
impl SkillsSearchHttp {
    pub fn new(manual_proxy: Option<&str>) -> Self {
        let mut config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .http_status_as_error(false)
            .user_agent(concat!("lyrel/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) =
            crate::daemon::install::proxy::resolve("https://skills.sh", manual_proxy)
        {
            config = config.proxy(Some(proxy));
        }
        Self {
            agent: config.build().into(),
        }
    }
}
impl Transport for SkillsSearchHttp {
    fn get(&self, path: &str) -> Result<crate::core::github::Reply, ApiError> {
        if !path.starts_with("/api/search?q=") || path.contains(['\r', '\n', '#']) {
            return Err(ApiError::Decode("Unsupported skills.sh request".into()));
        }
        let mut response = self
            .agent
            .get(format!("https://skills.sh{path}"))
            .header("Accept", "application/json")
            .call()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        crate::core::github::api::classify(
            status,
            &|name| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            },
            &body,
        )?;
        Ok(crate::core::github::Reply {
            body,
            has_next: false,
        })
    }
    fn authenticated(&self) -> bool {
        false
    }
}
