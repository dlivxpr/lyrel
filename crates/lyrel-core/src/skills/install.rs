//! Explicitly authorized shared installs. Preview is read-only; only per-item
//! decisions enter the staged Host write path. No agent-private directories exist here.
use super::{
    catalog::{GitHubSource, PreparedSkill},
    provenance::sanitize_name,
};
use crate::host::{Host, HostId, Meta};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io,
    path::{Component, Path, PathBuf},
};

const MAX_FILES: usize = 1_000;
const MAX_BYTES: u64 = 50 * 1024 * 1024;
const MAX_LOCK: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallScope {
    User,
    Project,
}
#[derive(Clone, Debug)]
pub struct InstallTarget {
    scope: InstallScope,
    anchor: PathBuf,
    shared_root: PathBuf,
    lock_path: PathBuf,
}
impl InstallTarget {
    /// Selectors are supplied by the caller, never read from process environment.
    /// A nonempty relative XDG_STATE_HOME is resolved against the selected cwd,
    /// matching the CLI, not against home.
    pub fn user(home: PathBuf, xdg_state_home: Option<PathBuf>, cwd: PathBuf) -> io::Result<Self> {
        validate_selector(&home)?;
        let lock_path = if let Some(state) = xdg_state_home.filter(|p| !p.as_os_str().is_empty()) {
            let state = if state.is_absolute() {
                state
            } else {
                validate_selector(&cwd)?;
                cwd.join(state)
            };
            normalize_selector(&state)?.join("skills/.skill-lock.json")
        } else {
            home.join(".agents/.skill-lock.json")
        };
        Ok(Self {
            scope: InstallScope::User,
            shared_root: home.join(".agents/skills"),
            anchor: home,
            lock_path,
        })
    }
    pub fn project(project: PathBuf) -> io::Result<Self> {
        validate_selector(&project)?;
        Ok(Self {
            scope: InstallScope::Project,
            shared_root: project.join(".agents/skills"),
            lock_path: project.join("skills-lock.json"),
            anchor: project,
        })
    }
    pub fn scope(&self) -> InstallScope {
        self.scope
    }
    pub fn shared_root(&self) -> &Path {
        &self.shared_root
    }
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
    pub fn anchor(&self) -> &Path {
        &self.anchor
    }
    fn version(&self) -> u64 {
        if self.scope == InstallScope::User {
            3
        } else {
            1
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExistingContent {
    Absent,
    Directory { files: usize, bytes: u64 },
}
#[derive(Clone, Debug)]
pub struct InstallPreviewItem {
    skill: PreparedSkill,
    destination: PathBuf,
    existing: ExistingContent,
    fingerprint: Option<String>,
}
impl InstallPreviewItem {
    pub fn name(&self) -> &str {
        &self.skill.candidate.name
    }
    pub fn destination(&self) -> &Path {
        &self.destination
    }
    pub fn existing(&self) -> &ExistingContent {
        &self.existing
    }
    pub fn source(&self) -> &GitHubSource {
        &self.skill.candidate.source
    }
    pub fn skill_path(&self) -> &str {
        &self.skill.candidate.skill_path
    }
}
#[derive(Clone, Debug)]
pub struct InstallPreview {
    host: HostId,
    target: InstallTarget,
    anchor_meta: Meta,
    items: Vec<InstallPreviewItem>,
    lock_bytes: Option<Vec<u8>>,
}
impl InstallPreview {
    pub fn target(&self) -> &InstallTarget {
        &self.target
    }
    pub fn items(&self) -> &[InstallPreviewItem] {
        &self.items
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallDecision {
    #[default]
    Keep,
    Install,
    Replace,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallStatus {
    Installed,
    Replaced,
    Kept,
    Failed(String),
}
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct InstallItemResult {
    pub name: String,
    pub destination: PathBuf,
    pub status: InstallStatus,
}
#[non_exhaustive]
#[derive(Clone, Debug, Default)]
pub struct InstallReport {
    pub items: Vec<InstallItemResult>,
    /// Cleanup failures are visible even when content and lock committed.
    pub warnings: Vec<String>,
}

/// Blocking, read-only. Dropping this value cancels with zero writes.
pub fn preview_install(
    host: &dyn Host,
    target: InstallTarget,
    skills: Vec<PreparedSkill>,
) -> io::Result<InstallPreview> {
    crate::host::guard_off_ui();
    check_host(host)?;
    if !host.is_absolute(&target.anchor)
        || !host.is_absolute(&target.shared_root)
        || !host.is_absolute(&target.lock_path)
    {
        return Err(invalid(
            "installation selectors must be absolute on the selected Host",
        ));
    }
    check_ancestors(host, &target.shared_root)?;
    check_ancestors(host, &target.lock_path)?;
    let anchor_meta = safe_meta(host, &target.anchor)?
        .ok_or_else(|| invalid("selected home/project does not exist"))?;
    if !anchor_meta.is_dir {
        return Err(invalid("selected home/project is not a directory"));
    }
    let (lock_bytes, _) = read_lock(host, &target)?;
    let mut names = HashSet::new();
    let mut directories = HashSet::new();
    let mut items = Vec::with_capacity(skills.len());
    let file_count = skills.iter().fold(0usize, |total, skill| {
        total.saturating_add(skill.files.len())
    });
    let byte_count = skills
        .iter()
        .flat_map(|skill| &skill.files)
        .fold(0u64, |total, file| {
            total.saturating_add(file.contents.len() as u64)
        });
    if file_count > MAX_FILES || byte_count > MAX_BYTES {
        return Err(invalid(
            "selected installation exceeds the selection budget of 1,000 files or 50 MiB",
        ));
    }
    for skill in skills {
        validate_skill(&skill)?;
        let name = sanitize_name(&skill.candidate.name);
        validate_relative(Path::new(&name))?;
        if !names.insert(skill.candidate.name.clone()) || !directories.insert(name.to_lowercase()) {
            return Err(invalid(
                "selected skills collide at the same installation or lock key",
            ));
        }
        let destination = host.join(&target.shared_root, &name);
        let (existing, fingerprint) = inspect_directory(host, &destination)?;
        items.push(InstallPreviewItem {
            skill,
            destination,
            existing,
            fingerprint,
        });
    }
    Ok(InstallPreview {
        host: host.id(),
        target,
        anchor_meta,
        items,
        lock_bytes,
    })
}

/// A missing decision means Keep, never implicit overwrite. Each successful item
/// commits its content and source record together; failed items are rolled back.
pub fn apply_install(
    host: &dyn Host,
    preview: InstallPreview,
    decisions: &[InstallDecision],
    installed_at: &str,
) -> InstallReport {
    crate::host::guard_off_ui();
    let mut report = InstallReport::default();
    let active = preview
        .items
        .iter()
        .enumerate()
        .any(|(i, _)| decisions.get(i).copied().unwrap_or_default() != InstallDecision::Keep);
    if !active {
        report.items = preview
            .items
            .iter()
            .map(|item| result(item, InstallStatus::Kept))
            .collect();
        return report;
    }
    let preflight = (|| {
        check_host(host)?;
        if host.id() != preview.host {
            return Err(invalid("installation confirmation belongs to another Host"));
        }
        if chrono::DateTime::parse_from_rfc3339(installed_at).is_err() {
            return Err(invalid("installation timestamp must be RFC 3339"));
        }
        check_ancestors(host, &preview.target.shared_root)?;
        check_ancestors(host, &preview.target.lock_path)?;
        if safe_meta(host, &preview.target.anchor)? != Some(preview.anchor_meta) {
            return Err(changed("selected home/project changed after confirmation"));
        }
        let (bytes, _) = read_lock(host, &preview.target)?;
        if bytes != preview.lock_bytes {
            return Err(changed("source lock changed after confirmation"));
        }
        for (index, item) in preview.items.iter().enumerate() {
            if decisions.get(index).copied().unwrap_or_default() != InstallDecision::Keep {
                recheck_item(host, item)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = preflight {
        for (i, item) in preview.items.iter().enumerate() {
            let status = if decisions.get(i).copied().unwrap_or_default() == InstallDecision::Keep {
                InstallStatus::Kept
            } else {
                InstallStatus::Failed(error.to_string())
            };
            report.items.push(result(item, status));
        }
        return report;
    }
    let mut expected_lock = preview.lock_bytes.clone();
    for (index, item) in preview.items.iter().enumerate() {
        let decision = decisions.get(index).copied().unwrap_or_default();
        if decision == InstallDecision::Keep {
            report.items.push(result(item, InstallStatus::Kept));
            continue;
        }
        let permitted = matches!(
            (&item.existing, decision),
            (ExistingContent::Absent, InstallDecision::Install)
                | (ExistingContent::Directory { .. }, InstallDecision::Replace)
        );
        if !permitted {
            report.items.push(result(
                item,
                InstallStatus::Failed(
                    "this item requires its own install/replace confirmation".into(),
                ),
            ));
            continue;
        }
        match install_one(
            host,
            &preview.target,
            item,
            &expected_lock,
            installed_at,
            &mut report.warnings,
        ) {
            Ok(bytes) => {
                expected_lock = Some(bytes);
                report.items.push(result(
                    item,
                    if decision == InstallDecision::Replace {
                        InstallStatus::Replaced
                    } else {
                        InstallStatus::Installed
                    },
                ));
            }
            Err(error) => report
                .items
                .push(result(item, InstallStatus::Failed(error.to_string()))),
        }
    }
    report
}
fn result(item: &InstallPreviewItem, status: InstallStatus) -> InstallItemResult {
    InstallItemResult {
        name: item.name().into(),
        destination: item.destination.clone(),
        status,
    }
}
fn check_host(host: &dyn Host) -> io::Result<()> {
    if host.is_connected() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "selected installation Host disconnected",
        ))
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn changed(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}
fn validate_selector(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid(
            "home/project selectors must be explicit paths without dot segments",
        ));
    }
    Ok(())
}
fn normalize_selector(path: &Path) -> io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => (),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(invalid("state selector escapes its filesystem root"));
                }
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    Ok(normalized)
}
/// Host::stat follows broken links on some peers. Read the parent entry as well,
/// so a dangling link is never confused with an absent destination.
fn safe_meta(host: &dyn Host, path: &Path) -> io::Result<Option<Meta>> {
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        match host.read_dir(parent, None) {
            Ok(entries) => {
                if entries
                    .iter()
                    .any(|e| e.name == name.to_string_lossy() && e.is_symlink)
                {
                    return Err(invalid(format!("refusing linked path: {}", path.display())));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    match host.stat(path) {
        Ok(meta) if meta.is_symlink => {
            Err(invalid(format!("refusing linked path: {}", path.display())))
        }
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn check_ancestors(host: &dyn Host, path: &Path) -> io::Result<()> {
    let chain: Vec<_> = path.ancestors().collect();
    for ancestor in chain.into_iter().rev() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if let Some(meta) = safe_meta(host, ancestor)? {
            if ancestor != path && !meta.is_dir {
                return Err(invalid(format!(
                    "ancestor is not a directory: {}",
                    ancestor.display()
                )));
            }
        }
    }
    Ok(())
}
fn validate_relative(path: &Path) -> io::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid("skill file path must be UTF-8"))?;
    if value.is_empty() || value.contains('\\') || path.is_absolute() {
        return Err(invalid("skill file path must be a relative portable path"));
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
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
        {
            return Err(invalid(format!("unsafe skill path: {value}")));
        }
    }
    Ok(value.into())
}
fn validate_skill(skill: &PreparedSkill) -> io::Result<()> {
    if skill.candidate.name.trim().is_empty() || skill.files.len() > MAX_FILES {
        return Err(invalid("invalid skill name or too many files"));
    }
    if skill.candidate.tree_hash.len() != 40
        || !skill
            .candidate
            .tree_hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || skill.computed_hash.len() != 64
        || !skill.computed_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("skill lacks CLI-compatible tree/content hashes"));
    }
    let mut paths = HashSet::new();
    let mut total = 0u64;
    let mut has_markdown = false;
    for file in &skill.files {
        let path = validate_relative(&file.path)?;
        if !paths.insert(path.to_lowercase()) {
            return Err(invalid("colliding skill paths"));
        }
        total = total.saturating_add(file.contents.len() as u64);
        if total > MAX_BYTES {
            return Err(invalid("skill exceeds installation byte limit"));
        }
        if path == "SKILL.md" {
            has_markdown = true;
            if file.contents.len() > 1024 * 1024 {
                return Err(invalid("SKILL.md exceeds byte limit"));
            }
        }
    }
    if !has_markdown {
        return Err(invalid("prepared skill has no SKILL.md"));
    }
    Ok(())
}
fn read_lock(host: &dyn Host, target: &InstallTarget) -> io::Result<(Option<Vec<u8>>, Value)> {
    check_ancestors(host, &target.lock_path)?;
    let Some(meta) = safe_meta(host, &target.lock_path)? else {
        let value = if target.scope == InstallScope::User {
            json!({"version":3,"skills":{},"dismissed":{}})
        } else {
            json!({"version":1,"skills":{}})
        };
        return Ok((None, value));
    };
    if meta.is_dir {
        return Err(invalid("source lock is a directory"));
    }
    let bytes = host.read_file(&target.lock_path, MAX_LOCK)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("invalid source lock; not overwritten: {error}")))?;
    if value.get("version").and_then(Value::as_u64) != Some(target.version())
        || !value.get("skills").is_some_and(Value::is_object)
    {
        return Err(invalid(
            "unsupported source lock version/shape; not overwritten",
        ));
    }
    if value["skills"]
        .as_object()
        .unwrap()
        .values()
        .any(|entry| !entry.is_object())
    {
        return Err(invalid("invalid source lock entry; not overwritten"));
    }
    Ok((Some(bytes), value))
}
fn inspect_directory(
    host: &dyn Host,
    directory: &Path,
) -> io::Result<(ExistingContent, Option<String>)> {
    check_ancestors(host, directory)?;
    let Some(meta) = safe_meta(host, directory)? else {
        return Ok((ExistingContent::Absent, None));
    };
    if !meta.is_dir {
        return Err(invalid(format!(
            "installation target is not a directory: {}",
            directory.display()
        )));
    }
    let mut paths = Vec::new();
    let mut stack = vec![(directory.to_path_buf(), String::new())];
    let mut directories = 0usize;
    let mut directory_records = Vec::new();
    let mut total = 0u64;
    while let Some((dir, relative)) = stack.pop() {
        let directory_meta = safe_meta(host, &dir)?
            .ok_or_else(|| changed("installation directory vanished during inspection"))?;
        if !directory_meta.is_dir {
            return Err(changed("installation directory changed during inspection"));
        }
        directory_records.push((relative.clone(), directory_meta));
        directories += 1;
        if directories > MAX_FILES {
            return Err(invalid(
                "existing installation exceeds inspection directory limit",
            ));
        }
        for entry in host.read_dir(&dir, None)? {
            validate_relative(Path::new(&entry.name))?;
            let path = host.join(&dir, &entry.name);
            if entry.is_symlink {
                return Err(invalid(format!(
                    "linked installation entry: {}",
                    path.display()
                )));
            }
            let md = safe_meta(host, &path)?
                .ok_or_else(|| changed("installation entry vanished during inspection"))?;
            let rel = if relative.is_empty() {
                entry.name
            } else {
                format!("{relative}/{}", entry.name)
            };
            if md.is_dir {
                stack.push((path, rel));
            } else {
                total = total.saturating_add(md.len);
                if total > MAX_BYTES || paths.len() >= MAX_FILES {
                    return Err(invalid("existing installation exceeds inspection limits"));
                }
                paths.push((rel, md));
            }
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    let count = paths.len();
    let mut digest = Sha256::new();
    directory_records.sort_by(|a, b| a.0.cmp(&b.0));
    for (relative, metadata) in directory_records {
        digest.update(relative.len().to_le_bytes());
        digest.update(relative.as_bytes());
        fingerprint_meta(&mut digest, &metadata);
    }
    fingerprint_meta(&mut digest, &meta);
    for (relative, metadata) in paths {
        let path = relative
            .split('/')
            .fold(directory.to_path_buf(), |base, name| host.join(&base, name));
        check_ancestors(host, &path)?;
        if safe_meta(host, &path)? != Some(metadata) {
            return Err(changed("installation file changed during inspection"));
        }
        let bytes = host.read_file(&path, MAX_BYTES)?;
        if safe_meta(host, &path)? != Some(metadata) {
            return Err(changed("installation file changed during inspection"));
        }
        digest.update(relative.len().to_le_bytes());
        digest.update(relative.as_bytes());
        fingerprint_meta(&mut digest, &metadata);
        digest.update(bytes);
    }
    Ok((
        ExistingContent::Directory {
            files: count,
            bytes: total,
        },
        Some(crate::daemon::install::checksums::hex(
            &digest.finalize().into(),
        )),
    ))
}
fn fingerprint_meta(digest: &mut Sha256, meta: &Meta) {
    digest.update([
        meta.is_dir as u8,
        meta.is_symlink as u8,
        meta.readonly as u8,
    ]);
    digest.update(meta.len.to_le_bytes());
    if let Some(mtime) = meta.mtime {
        digest.update([1]);
        digest.update(mtime.secs.to_le_bytes());
        digest.update(mtime.nanos.to_le_bytes());
    } else {
        digest.update([0]);
    }
}
fn recheck_item(host: &dyn Host, item: &InstallPreviewItem) -> io::Result<()> {
    let (existing, fingerprint) = inspect_directory(host, &item.destination)?;
    if existing != item.existing || fingerprint != item.fingerprint {
        return Err(changed(
            "installation target changed after confirmation; review again",
        ));
    }
    Ok(())
}
fn ensure_directory(
    host: &dyn Host,
    directory: &Path,
    created: &mut Vec<PathBuf>,
) -> io::Result<()> {
    check_ancestors(host, directory)?;
    let mut missing = Vec::new();
    for ancestor in directory.ancestors() {
        if let Some(meta) = safe_meta(host, ancestor)? {
            if !meta.is_dir {
                return Err(invalid("installation parent is not a directory"));
            }
            break;
        }
        missing.push(ancestor.to_path_buf());
    }
    for path in missing.into_iter().rev() {
        check_ancestors(host, &path)?;
        host.create_dir(&path, false)?;
        created.push(path);
    }
    Ok(())
}
fn merge_lock(
    mut value: Value,
    target: &InstallTarget,
    skill: &PreparedSkill,
    now: &str,
) -> io::Result<Vec<u8>> {
    let entries = value["skills"].as_object_mut().unwrap();
    let entry = entries
        .entry(skill.candidate.name.clone())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    entry.insert(
        "source".into(),
        Value::String(skill.candidate.source.repository()),
    );
    entry.insert("sourceType".into(), Value::String("github".into()));
    entry.insert(
        "skillPath".into(),
        Value::String(skill.candidate.skill_path.clone()),
    );
    if let Some(reference) = &skill.candidate.source.reference {
        entry.insert("ref".into(), Value::String(reference.clone()));
    } else {
        entry.remove("ref");
    }
    if target.scope == InstallScope::User {
        entry.insert(
            "sourceUrl".into(),
            Value::String(skill.candidate.source.url()),
        );
        entry.insert(
            "skillFolderHash".into(),
            Value::String(skill.candidate.tree_hash.clone()),
        );
        entry
            .entry("installedAt")
            .or_insert_with(|| Value::String(now.into()));
        entry.insert("updatedAt".into(), Value::String(now.into()));
    } else {
        entry.remove("sourceUrl");
        entry.insert(
            "computedHash".into(),
            Value::String(skill.computed_hash.clone()),
        );
    }
    if target.scope == InstallScope::Project {
        entries.sort_keys();
    }
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(io::Error::other)?;
    if target.scope == InstallScope::Project {
        bytes.push(b'\n');
    }
    if bytes.len() as u64 > MAX_LOCK {
        return Err(invalid("merged source lock exceeds safe read limit"));
    }
    Ok(bytes)
}
fn excluded(path: &Path) -> bool {
    path.components().any(|component| matches!(component, Component::Normal(name) if name == ".git" || name == "__pycache__" || name == "__pypackages__"))
        || path.file_name().is_some_and(|name| name == "metadata.json")
}
fn install_one(
    host: &dyn Host,
    target: &InstallTarget,
    item: &InstallPreviewItem,
    expected_lock: &Option<Vec<u8>>,
    now: &str,
    warnings: &mut Vec<String>,
) -> io::Result<Vec<u8>> {
    recheck_item(host, item)?;
    let (bytes, lock) = read_lock(host, target)?;
    if &bytes != expected_lock {
        return Err(changed("source lock changed; review again"));
    }
    let merged = merge_lock(lock, target, &item.skill, now)?;
    let token = uuid::Uuid::new_v4().to_string();
    let stage = host.join(&target.shared_root, &format!(".lyrel-stage-{token}"));
    let backup = host.join(&target.shared_root, &format!(".lyrel-backup-{token}"));
    let lock_parent = target
        .lock_path
        .parent()
        .ok_or_else(|| invalid("lock has no parent"))?;
    let lock_stage = host.join(lock_parent, &format!(".lyrel-lock-{token}.stage"));
    let lock_backup = host.join(lock_parent, &format!(".lyrel-lock-{token}.backup"));
    let write_guard = host.join(lock_parent, ".lyrel-skills-write.lock");
    let mut created = Vec::new();
    let mut stage_created = false;
    let mut lock_stage_created = false;
    let mut guard_created = false;
    let mut old_content_moved = false;
    let mut new_content_moved = false;
    let mut new_content_fingerprint = None;
    let mut old_lock_moved = false;
    let mut committed = false;
    let operation = (|| {
        ensure_directory(host, &target.shared_root, &mut created)?;
        ensure_directory(host, lock_parent, &mut created)?;
        check_ancestors(host, &write_guard)?;
        host.create_file_new(&write_guard)?;
        guard_created = true;
        // Another lyrel install may have finished while directories were prepared.
        let (current, _) = read_lock(host, target)?;
        if &current != expected_lock {
            return Err(changed("source lock changed before staging; review again"));
        }
        recheck_item(host, item)?;
        check_ancestors(host, &stage)?;
        host.create_dir(&stage, false)?;
        stage_created = true;
        for file in &item.skill.files {
            if excluded(&file.path) {
                continue;
            }
            let path = file
                .path
                .to_str()
                .ok_or_else(|| invalid("non UTF-8 prepared path"))?
                .split('/')
                .fold(stage.clone(), |base, name| host.join(&base, name));
            ensure_directory(host, path.parent().unwrap(), &mut Vec::new())?;
            check_ancestors(host, &path)?;
            host.create_file_new(&path)?;
            host.write_file(&path, &file.contents)?;
        }
        check_ancestors(host, &lock_stage)?;
        host.create_file_new(&lock_stage)?;
        lock_stage_created = true;
        host.write_file(&lock_stage, &merged)?;
        // Confirmation is checked again after slow staging, before either live rename.
        recheck_item(host, item)?;
        let (current, _) = read_lock(host, target)?;
        if &current != expected_lock {
            return Err(changed("source lock changed while staging; review again"));
        }
        check_ancestors(host, &target.shared_root)?;
        if !matches!(item.existing, ExistingContent::Absent) {
            host.rename(&item.destination, &backup)?;
            old_content_moved = true;
        }
        host.rename(&stage, &item.destination)?;
        stage_created = false;
        new_content_moved = true;
        new_content_fingerprint = inspect_directory(host, &item.destination)?.1;
        let (current, _) = read_lock(host, target)?;
        if &current != expected_lock {
            return Err(changed("source lock changed before commit; review again"));
        }
        if expected_lock.is_some() {
            host.rename(&target.lock_path, &lock_backup)?;
            old_lock_moved = true;
        }
        host.rename(&lock_stage, &target.lock_path)?;
        lock_stage_created = false;
        committed = true;
        Ok(())
    })();
    let mut rollback_errors = Vec::new();
    if !committed {
        if old_lock_moved {
            if let Err(e) = host.rename(&lock_backup, &target.lock_path) {
                rollback_errors.push(format!(
                    "source lock backup retained at {}: {e}",
                    lock_backup.display()
                ));
            }
        }
        if new_content_moved {
            match inspect_directory(host, &item.destination) {
                Ok((_, fingerprint))
                    if new_content_fingerprint.is_some()
                        && fingerprint == new_content_fingerprint =>
                {
                    if let Err(e) = host.remove(&item.destination, true) {
                        rollback_errors.push(format!(
                            "unrecorded new content remains at {}: {e}",
                            item.destination.display()
                        ));
                    }
                }
                _ => rollback_errors.push(format!(
                    "unrecorded content at {} changed after staging; not removed",
                    item.destination.display()
                )),
            }
        }
        if old_content_moved {
            if let Err(e) = host.rename(&backup, &item.destination) {
                rollback_errors.push(format!(
                    "original content backup retained at {}: {e}",
                    backup.display()
                ));
            }
        }
    } else {
        if old_lock_moved {
            cleanup(host, &lock_backup, false, warnings);
        }
        if old_content_moved {
            cleanup(host, &backup, true, warnings);
        }
    }
    if stage_created {
        cleanup(host, &stage, true, warnings);
    }
    if lock_stage_created {
        cleanup(host, &lock_stage, false, warnings);
    }
    if guard_created {
        cleanup(host, &write_guard, false, warnings);
    }
    if !committed {
        for path in created.into_iter().rev() {
            cleanup(host, &path, false, warnings);
        }
    }
    match operation {
        Ok(()) => Ok(merged),
        Err(error) if rollback_errors.is_empty() => Err(error),
        Err(error) => Err(io::Error::other(format!(
            "{error}; rollback incomplete: {}",
            rollback_errors.join("; ")
        ))),
    }
}
fn cleanup(host: &dyn Host, path: &Path, recursive: bool, warnings: &mut Vec<String>) {
    if let Err(error) = host.remove(path, recursive) {
        warnings.push(format!("cleanup {}: {error}", path.display()));
    }
}
