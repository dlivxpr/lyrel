//! Host-bound, reviewed maintenance of one concrete shared installation.
//! HTTP runs on the desktop. Previews are read-only and own every confirmation
//! input; cancellation is simply dropping the value. Native configuration and
//! private skill contents are never written.
use super::{
    ScanRequest, SkillInstallation, SourceKind, agent_roots,
    catalog::{self, GitHubSource, SkillFile},
    install::{self, ExistingContent, InstallPreview, InstallScope, InstallTarget},
    provenance::sanitize_name,
};
use crate::{
    core::{
        agent_hooks::{HookAgent, HookTarget},
        github::Transport,
    },
    host::{Host, HostId, Meta},
};
use install::{
    changed, check_ancestors, check_host, cleanup, inspect_directory, invalid, normalize_selector,
    parent_and_name, read_lock, rename_live, safe_meta, validate_relative,
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Available,
    UpToDate,
    DeletedUpstream,
    Failed(String),
    UnknownSource(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalChanges {
    Unmodified,
    Modified,
    UnknownBaseline,
}
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct UpdateCheck {
    pub status: UpdateStatus,
    pub local_changes: LocalChanges,
    /// Only Available produces a review. Missing decisions remain Keep, even
    /// with an unmodified baseline. Replace is a whole-directory replacement.
    pub preview: Option<InstallPreview>,
    pub warnings: Vec<String>,
}

/// Reuses the exact effective native profile discovery used for browsing. A
/// failed profile/configuration lookup is not silently replaced by GUI defaults.
pub fn private_roots(host: &dyn Host, request: &ScanRequest) -> io::Result<Vec<PathBuf>> {
    crate::host::guard_off_ui();
    check_host(host)?;
    if !host.is_absolute(&request.home)
        || request
            .project
            .as_deref()
            .is_some_and(|p| !host.is_absolute(p))
    {
        return Err(invalid(
            "skill selectors must be absolute on the selected Host",
        ));
    }
    let target = HookTarget::remote(host, request.home.clone())
        .with_launch_commands(&request.launch_commands);
    let mut roots = Vec::new();
    let mut problems = Vec::new();
    for agent in HookAgent::ALL {
        for root in agent_roots(host, &target, request, agent, &mut problems)? {
            if root.kind == SourceKind::AgentPrivate && !roots.contains(&root.path) {
                roots.push(root.path);
            }
        }
    }
    if !problems.is_empty() {
        return Err(invalid(problems.join("; ")));
    }
    Ok(roots)
}

struct Identity {
    destination: PathBuf,
    lock_key: Option<String>,
    lock_bytes: Option<Vec<u8>>,
    lock: Value,
    fingerprint: Option<String>,
    existing: ExistingContent,
    anchor_meta: Meta,
}
fn identity(
    host: &dyn Host,
    target: &InstallTarget,
    skill: &SkillInstallation,
) -> io::Result<Identity> {
    check_host(host)?;
    if host.id() != target.host() || host.id() != skill.host() {
        return Err(invalid("maintenance confirmation belongs to another Host"));
    }
    check_ancestors(host, target.shared_root())?;
    check_ancestors(host, target.lock_path())?;
    let anchor_meta = safe_meta(host, target.anchor())?
        .ok_or_else(|| changed("selected home/project disappeared"))?;
    let selected = normalize_selector(host, skill.path(), false)?;
    let (parent, name) =
        parent_and_name(host, &selected)?.ok_or_else(|| invalid("installation has no parent"))?;
    // Canonical spelling may carry a Windows verbatim prefix. Match using the
    // selected Host's semantics, never Path::parent on the desktop platform.
    if !paths_equal(
        host,
        &host.canonicalize(&parent)?,
        &host.canonicalize(target.shared_root())?,
    ) {
        return Err(invalid(
            "only a direct installation in the selected shared directory may be maintained",
        ));
    }
    validate_relative(Path::new(name))?;
    let destination = host.join(target.shared_root(), name);
    check_ancestors(host, &destination)?;
    if !paths_equal(host, &host.canonicalize(&destination)?, skill.path()) {
        return Err(changed(
            "selected installation path no longer identifies this installation",
        ));
    }
    let (existing, fingerprint) = inspect_directory(host, &destination)?;
    if matches!(existing, ExistingContent::Absent) {
        return Err(changed("selected installation disappeared"));
    }
    let (lock_bytes, lock) = read_lock(host, target)?;
    let keys: Vec<_> = lock["skills"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| {
            paths_equal(
                host,
                &host.join(target.shared_root(), &sanitize_name(key)),
                &destination,
            )
        })
        .cloned()
        .collect();
    if keys.len() > 1 {
        return Err(invalid(
            "multiple source lock keys identify this directory; preserve records and resolve the ambiguity first",
        ));
    }
    Ok(Identity {
        destination,
        lock_key: keys.into_iter().next(),
        lock_bytes,
        lock,
        fingerprint,
        existing,
        anchor_meta,
    })
}
fn source(entry: &Value) -> Result<(GitHubSource, String), String> {
    if entry.get("sourceType").and_then(Value::as_str) != Some("github") {
        return Err("No verifiable GitHub source record; no update address inferred".into());
    }
    let repository = entry
        .get("source")
        .and_then(Value::as_str)
        .ok_or("Missing source repository")?;
    let mut source = GitHubSource::parse(repository).map_err(|e| e.to_string())?;
    if source.repository() != repository
        || source.reference.is_some()
        || source.subpath.is_some()
        || source.skill_filter.is_some()
    {
        return Err("Source must identify an explicit GitHub owner/repository".into());
    }
    if let Some(reference) = entry.get("ref") {
        let reference = reference.as_str().ok_or("Invalid recorded ref")?;
        source = source
            .with_selection(Some(reference), None)
            .map_err(|e| e.to_string())?;
    }
    let path = entry
        .get("skillPath")
        .and_then(Value::as_str)
        .ok_or("Missing recorded skillPath; no path inferred")?;
    validate_relative(Path::new(path)).map_err(|e| e.to_string())?;
    if path.rsplit('/').next() != Some("SKILL.md") {
        return Err("Recorded skillPath does not identify SKILL.md".into());
    }
    if let Some(url) = entry.get("sourceUrl") {
        let parsed = GitHubSource::parse(url.as_str().ok_or("Invalid recorded sourceUrl")?)
            .map_err(|e| e.to_string())?;
        if parsed.repository() != source.repository()
            || parsed
                .reference
                .as_ref()
                .is_some_and(|reference| Some(reference) != source.reference.as_ref())
            || parsed.subpath.as_ref().is_some_and(|subpath| {
                !path
                    .strip_prefix(subpath)
                    .is_some_and(|tail| tail.starts_with('/'))
            })
            || parsed.skill_filter.is_some()
        {
            return Err("Recorded source URL and repository/ref disagree".into());
        }
    }
    source.subpath = path.rsplit_once('/').map(|(parent, _)| parent.to_string());
    Ok((source, path.to_string()))
}
fn hash_directory(host: &dyn Host, directory: &Path) -> io::Result<(String, bool)> {
    let mut files = Vec::new();
    let mut pending = vec![(directory.to_path_buf(), String::new())];
    let mut count = 0usize;
    let mut bytes = 0u64;
    let mut untracked_exclusions = false;
    while let Some((dir, relative)) = pending.pop() {
        count += 1;
        if count > 1_000 {
            return Err(invalid("installation hash directory limit reached"));
        }
        check_ancestors(host, &dir)?;
        for entry in host.read_dir(&dir, None)? {
            untracked_exclusions |= matches!(entry.name.as_str(), ".git" | "node_modules");
            validate_relative(Path::new(&entry.name))?;
            let path = host.join(&dir, &entry.name);
            let meta = safe_meta(host, &path)?
                .ok_or_else(|| changed("installation changed while hashing"))?;
            let rel = if relative.is_empty() {
                entry.name
            } else {
                format!("{relative}/{}", entry.name)
            };
            if meta.is_dir {
                pending.push((path, rel));
            } else {
                bytes = bytes.saturating_add(meta.len);
                if bytes > 50 * 1024 * 1024 || files.len() >= 1_000 {
                    return Err(invalid("installation hash budget reached"));
                }
                let contents = host.read_file(&path, 50 * 1024 * 1024)?;
                if safe_meta(host, &path)? != Some(meta) {
                    return Err(changed("installation changed while hashing"));
                }
                files.push(SkillFile::new(rel.into(), contents));
            }
        }
    }
    Ok((
        catalog::computed_hash(&files).map_err(io::Error::other)?,
        untracked_exclusions,
    ))
}
fn valid_hash(value: Option<&Value>, len: usize) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Blocking desktop-worker operation. A missing/deleted/private repository is a
/// check failure, not proof that an installed skill was deleted upstream. Only
/// absence from a complete, successfully fetched tree proves DeletedUpstream.
pub fn check_update(
    host: &dyn Host,
    target: InstallTarget,
    skill: &SkillInstallation,
    http: &dyn Transport,
) -> UpdateCheck {
    crate::host::guard_off_ui();
    let mut result = UpdateCheck {
        status: UpdateStatus::Available,
        local_changes: LocalChanges::UnknownBaseline,
        preview: None,
        warnings: Vec::new(),
    };
    let operation = (|| -> io::Result<()> {
        let identity = identity(host, &target, skill)?;
        let Some(key) = &identity.lock_key else {
            result.status =
                UpdateStatus::UnknownSource("No matching source record; no source inferred".into());
            return Ok(());
        };
        let entry = &identity.lock["skills"][key];
        let (source, path) = match source(entry) {
            Ok(source) => source,
            Err(reason) => {
                result.status = UpdateStatus::UnknownSource(reason);
                return Ok(());
            }
        };
        let baseline = valid_hash(entry.get("contentHash"), 64).or_else(|| {
            (target.scope() == InstallScope::Project)
                .then(|| valid_hash(entry.get("computedHash"), 64))
                .flatten()
        });
        if let Some(baseline) = baseline {
            let (hash, untracked_exclusions) = hash_directory(host, &identity.destination)?;
            result.local_changes = if !untracked_exclusions && hash == baseline {
                LocalChanges::Unmodified
            } else {
                LocalChanges::Modified
            };
        }
        let catalog = catalog::discover(http, &source).map_err(io::Error::other)?;
        result.warnings.extend(catalog.problems().iter().cloned());
        let Some(index) = catalog
            .candidates()
            .iter()
            .position(|candidate| candidate.skill_path == path)
        else {
            result.status = if catalog.contains_skill_path(&path) {
                UpdateStatus::Failed(
                    "Recorded upstream SKILL.md exists but is not a valid installable skill".into(),
                )
            } else {
                UpdateStatus::DeletedUpstream
            };
            return Ok(());
        };
        let prepared = catalog.prepare(http, index).map_err(io::Error::other)?;
        let up_to_date = if target.scope() == InstallScope::User {
            valid_hash(entry.get("skillFolderHash"), 40)
                == Some(prepared.candidate.tree_hash.as_str())
        } else {
            valid_hash(entry.get("computedHash"), 64) == Some(prepared.computed_hash.as_str())
        };
        if inspect_directory(host, &identity.destination)?
            != (identity.existing, identity.fingerprint)
        {
            return Err(changed(
                "installation changed while checking updates; review again",
            ));
        }
        if read_lock(host, &target)?.0 != identity.lock_bytes {
            return Err(changed("source lock changed while checking updates"));
        }
        if up_to_date {
            result.status = UpdateStatus::UpToDate;
        } else {
            result.preview = Some(install::preview_update(
                host,
                target,
                prepared,
                key,
                &identity.destination,
                &identity.lock_bytes,
            )?);
        }
        Ok(())
    })();
    if let Err(error) = operation {
        result.status = UpdateStatus::Failed(error.to_string());
        result.preview = None;
    }
    result
}

#[derive(Clone, Debug)]
pub struct RemovalLink {
    path: PathBuf,
    target: PathBuf,
    parent: PathBuf,
    parent_meta: Meta,
}
impl RemovalLink {
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Raw Host::read_link target, including relative spelling or junction prefix.
    pub fn target(&self) -> &Path {
        &self.target
    }
}
#[derive(Clone, Debug)]
pub struct RemovePreview {
    host: HostId,
    target: InstallTarget,
    destination: PathBuf,
    lock_key: Option<String>,
    lock_bytes: Option<Vec<u8>>,
    lock: Value,
    existing: ExistingContent,
    fingerprint: Option<String>,
    anchor_meta: Meta,
    links: Vec<RemovalLink>,
    warnings: Vec<String>,
}
impl RemovePreview {
    pub fn target(&self) -> &InstallTarget {
        &self.target
    }
    pub fn destination(&self) -> &Path {
        &self.destination
    }
    pub fn lock_key(&self) -> Option<&str> {
        self.lock_key.as_deref()
    }
    pub fn links(&self) -> &[RemovalLink] {
        &self.links
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalStatus {
    Removed,
    Retained(String),
    Failed(String),
    OutcomeUnknown(String),
}
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct RemovalLinkResult {
    pub path: PathBuf,
    pub status: RemovalStatus,
}
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct RemoveReport {
    pub content: RemovalStatus,
    pub source_record: RemovalStatus,
    pub links: Vec<RemovalLinkResult>,
    pub warnings: Vec<String>,
}

fn path_key(host: &dyn Host, path: &Path) -> String {
    let text = path.to_string_lossy();
    if host.separator() != '\\' {
        return text.trim_end_matches('/').to_string();
    }
    let text = text.replace('/', "\\").to_lowercase();
    let text = text
        .strip_prefix(r"\\?\unc\")
        .map(|tail| format!(r"\\{tail}"))
        .unwrap_or_else(|| text.strip_prefix(r"\\?\").unwrap_or(&text).to_string());
    text.trim_end_matches('\\').to_string()
}
fn paths_equal(host: &dyn Host, left: &Path, right: &Path) -> bool {
    path_key(host, left) == path_key(host, right)
}
fn resolved_link(host: &dyn Host, parent: &Path, raw: &Path) -> io::Result<PathBuf> {
    // Preserve raw confirmation bytes; normalize NT junction targets only for comparison.
    let raw = if host.separator() == '\\' {
        match raw.to_str().and_then(|value| value.strip_prefix(r"\??\")) {
            Some(tail) => std::borrow::Cow::Owned(PathBuf::from(format!(r"\\?\{tail}"))),
            None => std::borrow::Cow::Borrowed(raw),
        }
    } else {
        std::borrow::Cow::Borrowed(raw)
    };
    let path = if host.is_absolute(&raw) {
        raw.to_path_buf()
    } else {
        let raw = raw
            .to_str()
            .ok_or_else(|| invalid("non-UTF8 link target; retained"))?;
        if host.separator() == '\\' && (raw.contains(':') || raw.starts_with(['\\', '/'])) {
            return Err(invalid("ambiguous drive-relative link target; retained"));
        }
        host.join(parent, raw)
    };
    normalize_selector(host, &path, true)
}
fn collect_links(
    host: &dyn Host,
    roots: &[PathBuf],
    destination: &Path,
    warnings: &mut Vec<String>,
) -> Vec<RemovalLink> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    let mut pending = roots.to_vec();
    let mut budget = 8_000usize;
    while let Some(root) = pending.pop() {
        let root = match normalize_selector(host, &root, false) {
            Ok(root) => root,
            Err(error) => {
                warnings.push(format!("{}: {error}; retained", root.display()));
                continue;
            }
        };
        if !seen.insert(path_key(host, &root)) {
            continue;
        }
        // Never descend into the shared installation or follow a private root link.
        if paths_equal(host, &root, destination) {
            continue;
        }
        let scan = (|| -> io::Result<()> {
            check_ancestors(host, &root)?;
            let Some(meta) = safe_meta(host, &root)? else {
                return Ok(());
            };
            if !meta.is_dir {
                return Ok(());
            }
            for entry in host.read_dir(&root, None)? {
                if budget == 0 {
                    return Err(invalid(
                        "private link inspection limit reached; remaining entries retained",
                    ));
                }
                budget -= 1;
                if let Err(error) = validate_relative(Path::new(&entry.name)) {
                    warnings.push(format!(
                        "{} / {}: {error}; retained",
                        root.display(),
                        entry.name
                    ));
                    continue;
                }
                let path = host.join(&root, &entry.name);
                match host.read_link(&path) {
                    Ok(raw) => {
                        match resolved_link(host, &root, &raw) {
                            Ok(resolved) if paths_equal(host, &resolved, destination) => {
                                // The lexical target and real target must both agree.
                                match host.canonicalize(&path) {
                                    Ok(actual) if paths_equal(host, &actual, destination) => links
                                        .push(RemovalLink {
                                            path,
                                            target: raw,
                                            parent: root.clone(),
                                            parent_meta: meta,
                                        }),
                                    Ok(_) => warnings.push(format!(
                                        "{}: resolved link target differs; retained",
                                        path.display()
                                    )),
                                    Err(error) => warnings.push(format!(
                                        "{}: cannot prove link target: {error}; retained",
                                        path.display()
                                    )),
                                }
                            }
                            Ok(_) => {
                                if host
                                    .canonicalize(&path)
                                    .is_ok_and(|actual| paths_equal(host, &actual, destination))
                                {
                                    warnings.push(format!("{}: indirect link target cannot be pinned safely; retained",path.display()));
                                }
                            }
                            Err(error) => warnings.push(format!("{}: {error}", path.display())),
                        }
                    }
                    Err(error) if !entry.is_symlink => {
                        // Windows reports non-reparse entries with a platform
                        // error rather than InvalidInput. Metadata still proves
                        // these are real directories; linked entries never enter.
                        if error.kind() == io::ErrorKind::Unsupported {
                            warnings.push(format!(
                                "{}: cannot prove link target: {error}; linked entries retained",
                                path.display()
                            ));
                        }
                        match safe_meta(host, &path) {
                            Ok(Some(meta)) if meta.is_dir => pending.push(path),
                            Err(error) => {
                                warnings.push(format!("{}: {error}; retained", path.display()))
                            }
                            _ => (),
                        }
                    }
                    Err(error) => warnings.push(format!(
                        "{}: cannot prove link type/target: {error}; retained",
                        path.display()
                    )),
                }
            }
            Ok(())
        })();
        if let Err(error) = scan {
            warnings.push(format!(
                "{}: {error}; private entries retained",
                root.display()
            ));
        }
    }
    links
}

/// Read-only removal confirmation. Agent roots are the effective roots returned
/// by private_roots; aliases are found by target, never by installation name.
pub fn preview_remove(
    host: &dyn Host,
    target: InstallTarget,
    skill: &SkillInstallation,
    agent_roots: &[PathBuf],
) -> io::Result<RemovePreview> {
    crate::host::guard_off_ui();
    let identity = identity(host, &target, skill)?;
    let mut warnings = Vec::new();
    let links = collect_links(host, agent_roots, &identity.destination, &mut warnings);
    // Slow discovery must not produce a preview combining different snapshots.
    if inspect_directory(host, &identity.destination)?
        != (identity.existing.clone(), identity.fingerprint.clone())
        || read_lock(host, &target)?.0 != identity.lock_bytes
    {
        return Err(changed(
            "installation or source lock changed during removal preview",
        ));
    }
    Ok(RemovePreview {
        host: host.id(),
        target,
        destination: identity.destination,
        lock_key: identity.lock_key,
        lock_bytes: identity.lock_bytes,
        lock: identity.lock,
        existing: identity.existing,
        fingerprint: identity.fingerprint,
        anchor_meta: identity.anchor_meta,
        links,
        warnings,
    })
}
fn recheck(host: &dyn Host, preview: &RemovePreview, check_anchor: bool) -> io::Result<()> {
    check_host(host)?;
    if host.id() != preview.host {
        return Err(invalid("removal confirmation belongs to another Host"));
    }
    check_ancestors(host, preview.target.shared_root())?;
    check_ancestors(host, preview.target.lock_path())?;
    if (check_anchor && safe_meta(host, preview.target.anchor())? != Some(preview.anchor_meta))
        || inspect_directory(host, &preview.destination)?
            != (preview.existing.clone(), preview.fingerprint.clone())
        || read_lock(host, &preview.target)?.0 != preview.lock_bytes
    {
        return Err(changed(
            "confirmed installation, content, path, or source lock changed; review removal again",
        ));
    }
    Ok(())
}
fn recheck_link(
    host: &dyn Host,
    link: &RemovalLink,
    destination: &Path,
    require_live_target: bool,
) -> io::Result<()> {
    check_ancestors(host, &link.parent)?;
    if host.read_link(&link.path)? != link.target
        || !paths_equal(
            host,
            &resolved_link(host, &link.parent, &link.target)?,
            destination,
        )
    {
        return Err(changed("confirmed link changed; retained"));
    }
    if safe_meta(host, &link.parent)? != Some(link.parent_meta)
        || (require_live_target && !paths_equal(host, &host.canonicalize(&link.path)?, destination))
    {
        return Err(changed("confirmed link or its parent changed; retained"));
    }
    Ok(())
}
fn uncertain(error: &io::Error, host: &dyn Host) -> bool {
    !host.is_connected()
        || matches!(
            error.kind(),
            io::ErrorKind::NotConnected
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::TimedOut
                | io::ErrorKind::UnexpectedEof
        )
}

/// Consumes the exact confirmation. Partial/unknown results include recovery
/// paths and must be reviewed, not fed blindly into another destructive retry.
pub fn apply_remove(host: &dyn Host, preview: RemovePreview) -> RemoveReport {
    crate::host::guard_off_ui();
    let mut report = RemoveReport {
        content: RemovalStatus::Retained("Not removed".into()),
        source_record: RemovalStatus::Retained(
            if preview.lock_key.is_some() {
                "Source record unchanged"
            } else {
                "No matching source record; unchanged"
            }
            .into(),
        ),
        links: preview
            .links
            .iter()
            .map(|link| RemovalLinkResult {
                path: link.path.clone(),
                status: RemovalStatus::Retained("Not removed".into()),
            })
            .collect(),
        warnings: preview.warnings.clone(),
    };
    if let Err(error) = recheck(host, &preview, true) {
        report.content = RemovalStatus::Failed(error.to_string());
        return report;
    }
    let approved: Vec<_> = preview
        .links
        .iter()
        .enumerate()
        .filter_map(
            |(index, link)| match recheck_link(host, link, &preview.destination, true) {
                Ok(()) => Some(index),
                Err(error) => {
                    report.links[index].status = RemovalStatus::Retained(error.to_string());
                    None
                }
            },
        )
        .collect();
    let token = uuid::Uuid::new_v4();
    let tombstone = host.join(
        preview.target.shared_root(),
        &format!(".lyrel-remove-{token}"),
    );
    let lock_parent = match parent_and_name(host, preview.target.lock_path()) {
        Ok(Some((parent, _))) => parent,
        _ => {
            report.content = RemovalStatus::Failed("source lock has no safe parent".into());
            return report;
        }
    };
    let guard_parent = if preview.lock_bytes.is_some() {
        &lock_parent
    } else {
        preview.target.shared_root()
    };
    let guard = host.join(guard_parent, ".lyrel-skills-write.lock");
    let stage = host.join(&lock_parent, &format!(".lyrel-lock-{token}.stage"));
    let backup = host.join(&lock_parent, &format!(".lyrel-lock-{token}.backup"));
    let mut guard_created = false;
    let mut stage_created = false;
    let mut content_moved = false;
    let mut lock_moved = false;
    let mut lock_publication_started = false;
    let mut committed = false;
    let mut publication_uncertain = false;
    let operation = (|| -> io::Result<()> {
        check_ancestors(host, &guard)?;
        host.create_file_new(&guard)?;
        guard_created = true;
        if let Some(key) = &preview.lock_key {
            let mut lock = preview.lock.clone();
            lock["skills"].as_object_mut().unwrap().remove(key);
            if preview.target.scope() == InstallScope::Project {
                lock["skills"].as_object_mut().unwrap().sort_keys();
            }
            let mut bytes = serde_json::to_vec_pretty(&lock).map_err(io::Error::other)?;
            if preview.target.scope() == InstallScope::Project {
                bytes.push(b'\n');
            }
            check_ancestors(host, &stage)?;
            host.create_file_new(&stage)?;
            stage_created = true;
            host.write_file(&stage, &bytes)?;
        }
        // All user inputs are rechecked after staging and immediately before
        // publication; staging itself never changes a live installation.
        recheck(host, &preview, false)?;
        rename_live(
            host,
            &preview.destination,
            &tombstone,
            &mut publication_uncertain,
        )?;
        content_moved = true;
        if preview.lock_key.is_some() {
            if read_lock(host, &preview.target)?.0 != preview.lock_bytes {
                return Err(changed("source lock changed before removal commit"));
            }
            lock_publication_started = true;
            rename_live(
                host,
                preview.target.lock_path(),
                &backup,
                &mut publication_uncertain,
            )?;
            lock_moved = true;
            rename_live(
                host,
                &stage,
                preview.target.lock_path(),
                &mut publication_uncertain,
            )?;
            stage_created = false;
        }
        committed = true;
        Ok(())
    })();
    if publication_uncertain {
        let message = format!(
            "{}; removal outcome unknown. Reconnect and inspect installation {}, detached content {}, lock {}, lock stage {}, lock backup {}, guard {} before recovery; do not blindly repeat removal",
            operation.unwrap_err(),
            preview.destination.display(),
            tombstone.display(),
            preview.target.lock_path().display(),
            stage.display(),
            backup.display(),
            guard.display()
        );
        report.content = RemovalStatus::OutcomeUnknown(message.clone());
        if lock_publication_started {
            report.source_record = RemovalStatus::OutcomeUnknown(message);
        }
        return report;
    }
    if !committed {
        let mut errors = vec![operation.unwrap_err().to_string()];
        let mut recovery_uncertain = false;
        if lock_moved {
            let restore = (|| -> io::Result<()> {
                if safe_meta(host, preview.target.lock_path())?.is_some() {
                    return Err(changed(
                        "source lock appeared during recovery; not overwritten",
                    ));
                }
                rename_live(
                    host,
                    &backup,
                    preview.target.lock_path(),
                    &mut recovery_uncertain,
                )
            })();
            if let Err(error) = restore {
                if recovery_uncertain {
                    let message = format!(
                        "source lock recovery outcome unknown: {error}; inspect lock {}, backup {}, content {}, detached content {}, stage {} and guard {} before recovery; do not blindly repeat removal",
                        preview.target.lock_path().display(),
                        backup.display(),
                        preview.destination.display(),
                        tombstone.display(),
                        stage.display(),
                        guard.display()
                    );
                    report.source_record = RemovalStatus::OutcomeUnknown(message.clone());
                    report.content = RemovalStatus::OutcomeUnknown(message);
                    return report;
                }
                errors.push(format!(
                    "source lock backup retained at {}: {error}",
                    backup.display()
                ));
                report.source_record = RemovalStatus::Failed(errors.last().unwrap().clone());
            }
        }
        if content_moved {
            let restore = (|| -> io::Result<()> {
                check_ancestors(host, &preview.destination)?;
                if safe_meta(host, &preview.destination)?.is_some() {
                    return Err(changed(
                        "installation appeared during recovery; not overwritten",
                    ));
                }
                rename_live(
                    host,
                    &tombstone,
                    &preview.destination,
                    &mut recovery_uncertain,
                )
            })();
            if let Err(error) = restore {
                if recovery_uncertain {
                    report.content = RemovalStatus::OutcomeUnknown(format!(
                        "content recovery outcome unknown: {error}; inspect installation {}, detached content {}, lock {}, stage {} and guard {} before recovery; do not blindly repeat removal",
                        preview.destination.display(),
                        tombstone.display(),
                        preview.target.lock_path().display(),
                        stage.display(),
                        guard.display()
                    ));
                    return report;
                }
                errors.push(format!(
                    "detached original content retained at {}: {error}; inspect before retrying",
                    tombstone.display()
                ));
            }
        }
        report.content = RemovalStatus::Failed(errors.join("; "));
    } else {
        report.content = match inspect_directory(host, &tombstone) {
            Ok((existing, fingerprint))
                if existing == preview.existing && fingerprint == preview.fingerprint =>
            {
                match host.remove(&tombstone, true) {
                    Ok(()) => RemovalStatus::Removed,
                    Err(error) => {
                        let message = format!(
                            "installation detached; cleanup at {} failed: {error}; inspect this path before recovery, do not blindly repeat removal",
                            tombstone.display()
                        );
                        if uncertain(&error, host) {
                            RemovalStatus::OutcomeUnknown(message)
                        } else {
                            RemovalStatus::Failed(message)
                        }
                    }
                }
            }
            Ok(_) => RemovalStatus::Failed(format!(
                "detached content at {} changed; retained for review",
                tombstone.display()
            )),
            Err(error) => RemovalStatus::Failed(format!(
                "detached content at {} could not be verified; retained: {error}",
                tombstone.display()
            )),
        };
        if preview.lock_key.is_some() {
            report.source_record = RemovalStatus::Removed;
        }
        if lock_moved {
            cleanup(host, &backup, false, &mut report.warnings);
        }
        let mut parents: HashMap<PathBuf, Meta> = HashMap::new();
        for index in approved {
            let mut link = preview.links[index].clone();
            link.parent_meta = *parents.get(&link.parent).unwrap_or(&link.parent_meta);
            report.links[index].status = match recheck_link(
                host,
                &link,
                &preview.destination,
                false,
            ) {
                Err(error) => RemovalStatus::Retained(error.to_string()),
                Ok(()) => match host.remove(&link.path, false) {
                    Ok(()) => {
                        // Our own unlink changes the private directory's mtime.
                        // Establish its next proof only after known success;
                        // failed or uncertain unlinks cannot bless later edits.
                        if let Ok(Some(meta)) = safe_meta(host, &link.parent) {
                            parents.insert(link.parent.clone(), meta);
                        }
                        RemovalStatus::Removed
                    }
                    Err(error) if uncertain(&error, host) => RemovalStatus::OutcomeUnknown(
                        format!("{error}; inspect {} before retrying", link.path.display()),
                    ),
                    Err(error) => RemovalStatus::Failed(format!(
                        "{error}; installation was removed but this link remains at {}; review its raw target {} before unlinking; do not repeat the entire removal",
                        link.path.display(),
                        link.target.display()
                    )),
                },
            };
        }
    }
    if stage_created {
        cleanup(host, &stage, false, &mut report.warnings);
    }
    if guard_created {
        cleanup(host, &guard, false, &mut report.warnings);
    }
    report
}
