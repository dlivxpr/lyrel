#![cfg(feature = "github")]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use lyrel_core::{
    core::github::{ApiError, Reply, Transport},
    host::{self, Host, SharedHost, local::LocalHost},
    skills::{
        ScanRequest, SkillInstallation,
        catalog::{GitHubSource, discover},
        install::{InstallDecision, InstallStatus, InstallTarget, apply_install, preview_install},
        maintenance::{
            LocalChanges, RemovalStatus, UpdateStatus, apply_remove, check_update, preview_remove,
        },
        scan,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

const NOW: &str = "2026-10-19T08:00:00.000Z";
const OLD: &str = "---\nname: Original Name\ndescription: Original instructions\n---\n# Original\n";
const NEW: &str =
    "---\nname: Renamed Upstream\ndescription: New instructions\n---\n# Replacement\n";

// Only external HTTP varies: production catalog parsing, bounded downloads,
// source records, staging, and all filesystem effects remain real.
struct Http {
    document: &'static str,
    tree: &'static str,
    deleted: bool,
    error: Option<ApiError>,
}
impl Http {
    fn old() -> Self {
        Self {
            document: OLD,
            tree: "2222222222222222222222222222222222222222",
            deleted: false,
            error: None,
        }
    }
    fn new() -> Self {
        Self {
            document: NEW,
            tree: "4444444444444444444444444444444444444444",
            deleted: false,
            error: None,
        }
    }
}
impl Transport for Http {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let blob = "3333333333333333333333333333333333333333";
        let value = if path == "/repos/acme/tools/git/trees/release%2Fstable?recursive=1" {
            let entries = if self.deleted {
                vec![]
            } else {
                vec![
                    json!({"path":"skills","mode":"040000","type":"tree","sha":"1111111111111111111111111111111111111111"}),
                    json!({"path":"skills/original","mode":"040000","type":"tree","sha":self.tree}),
                    json!({"path":"skills/original/SKILL.md","mode":"100644","type":"blob","sha":blob,"size":self.document.len()}),
                ]
            };
            json!({"sha":"1111111111111111111111111111111111111111","truncated":false,"tree":entries})
        } else if path == format!("/repos/acme/tools/git/blobs/{blob}") {
            json!({"sha":blob,"size":self.document.len(),"encoding":"base64","content":STANDARD.encode(self.document)})
        } else {
            return Err(ApiError::NotFound);
        };
        Ok(Reply {
            body: serde_json::to_vec(&value).unwrap(),
            has_next: false,
        })
    }
    fn authenticated(&self) -> bool {
        false
    }
}

// A real isolated filesystem Host. Only selectors and explicitly injected I/O
// failures differ from LocalHost; no user configuration or skill roots are read.
struct SandboxHost {
    inner: SharedHost,
    home: PathBuf,
    deny_remove: Option<PathBuf>,
    deny_lock: bool,
    lose_remove_reply: bool,
    unsupported_links: bool,
    other_host: bool,
    retarget_on_detach: Option<(PathBuf, PathBuf)>,
}
impl Host for SandboxHost {
    fn id(&self) -> host::HostId {
        if self.other_host {
            host::HostId(105)
        } else {
            self.inner.id()
        }
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, p: &Path, r: Option<&Path>) -> io::Result<Vec<host::Entry>> {
        self.inner.read_dir(p, r)
    }
    fn stat(&self, p: &Path) -> io::Result<host::Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn read_link(&self, p: &Path) -> io::Result<PathBuf> {
        if self.unsupported_links {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "selected peer must upgrade read-link capability",
            ));
        }
        self.inner.read_link(p)
    }
    fn search(
        &self,
        r: &[PathBuf],
        q: &str,
        l: usize,
        d: usize,
        h: bool,
    ) -> io::Result<Vec<host::SearchHit>> {
        self.inner.search(r, q, l, d, h)
    }
    fn search_content(
        &self,
        r: &[PathBuf],
        q: &host::ContentQuery,
        l: &host::ContentLimits,
    ) -> io::Result<host::ContentResults> {
        self.inner.search_content(r, q, l)
    }
    fn write_file(&self, p: &Path, b: &[u8]) -> io::Result<host::Meta> {
        self.inner.write_file(p, b)
    }
    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.deny_lock
            && from.extension().is_some_and(|ext| ext == "stage")
            && to.file_name().is_some_and(|n| n == "skills-lock.json")
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "isolated source lock publication denied",
            ));
        }
        if self.lose_remove_reply
            && to
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".lyrel-remove-"))
        {
            self.inner.rename(from, to)?;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "publication happened; reply lost",
            ));
        }
        self.inner.rename(from, to)?;
        if to
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".lyrel-remove-"))
        {
            if let Some((alias, target)) = &self.retarget_on_detach {
                self.inner.remove(alias, false)?;
                link(target, alias);
            }
        }
        Ok(())
    }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        if self
            .deny_remove
            .as_deref()
            .is_some_and(|denied| p.starts_with(denied))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "isolated removal denied",
            ));
        }
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, p: &Path, a: &[&str]) -> io::Result<host::Output> {
        self.inner.git(p, a)
    }
    fn shells(&self) -> io::Result<host::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, p: &[PathBuf]) -> io::Result<host::WatchSub> {
        self.inner.watch(p)
    }
    fn agent_environment(&self) -> io::Result<BTreeMap<String, String>> {
        let h = self.home.to_string_lossy();
        Ok(BTreeMap::from([
            ("HOME".into(), h.to_string()),
            ("USERPROFILE".into(), h.to_string()),
            ("LYREL_HOST_OS".into(), std::env::consts::OS.into()),
            ("PROGRAMDATA".into(), format!("{h}/programdata")),
            ("CLAUDE_CONFIG_DIR".into(), format!("{h}/.claude")),
            ("XDG_CONFIG_HOME".into(), format!("{h}/.config")),
            ("XDG_STATE_HOME".into(), format!("{h}/.state")),
            ("XDG_DATA_HOME".into(), format!("{h}/.data")),
            ("PI_CONFIG_DIR".into(), format!("{h}/.omp")),
            ("PI_CODING_AGENT_DIR".into(), format!("{h}/.pi/agent")),
            ("OMP_PROFILE".into(), "default".into()),
            ("KIMI_CODE_HOME".into(), format!("{h}/.kimi")),
            ("HERMES_HOME".into(), format!("{h}/.hermes")),
            (
                "OPENCODE_CONFIG_DIR".into(),
                format!("{h}/.config/opencode"),
            ),
        ]))
    }
}
struct Fixture {
    _temp: tempfile::TempDir,
    host: SandboxHost,
    project: PathBuf,
    destination: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let host = SandboxHost {
            inner: LocalHost::new(),
            home,
            deny_remove: None,
            deny_lock: false,
            lose_remove_reply: false,
            unsupported_links: false,
            other_host: false,
            retarget_on_detach: None,
        };
        let destination = project.join(".agents/skills/original-name");
        let source = GitHubSource::parse("acme/tools#release%2Fstable").unwrap();
        let catalog = discover(&Http::old(), &source).unwrap();
        let prepared = catalog.prepare(&Http::old(), 0).unwrap();
        let target = InstallTarget::project(&host, project.clone()).unwrap();
        let preview = preview_install(&host, target, vec![prepared]).unwrap();
        assert_eq!(
            apply_install(&host, preview, &[InstallDecision::Install], NOW).items[0].status,
            InstallStatus::Installed
        );
        Self {
            _temp: temp,
            host,
            project,
            destination,
        }
    }
    fn target(&self) -> InstallTarget {
        InstallTarget::project(&self.host, self.project.clone()).unwrap()
    }
    fn skill(&self) -> SkillInstallation {
        let path = self.host.canonicalize(&self.destination).unwrap();
        scan(
            &self.host,
            &ScanRequest::new(&self.host.home).with_project(&self.project),
        )
        .unwrap()
        .installations()
        .iter()
        .find(|skill| skill.path() == path)
        .unwrap()
        .clone()
    }
    fn lock(&self) -> Value {
        serde_json::from_slice(&fs::read(self.project.join("skills-lock.json")).unwrap()).unwrap()
    }
}

#[test]
fn installed_modified_checked_kept_replaced_and_removed_without_losing_identity_or_extensions() {
    let f = Fixture::new();
    let mut lock = f.lock();
    lock["extension"] = json!({"keep":true});
    lock["skills"]["Original Name"]["future"] = json!(["preserve", 7]);
    lock["skills"]["unrelated"] = json!({"source":"../mine","sourceType":"local","opaque":true});
    fs::write(
        f.project.join("skills-lock.json"),
        serde_json::to_vec(&lock).unwrap(),
    )
    .unwrap();
    let before = fs::read(f.project.join("skills-lock.json")).unwrap();
    fs::write(f.destination.join("my-notes.txt"), "handwritten").unwrap();
    let checked = check_update(&f.host, f.target(), &f.skill(), &Http::new());
    assert_eq!(checked.status, UpdateStatus::Available);
    assert_eq!(checked.local_changes, LocalChanges::Modified);
    let kept = apply_install(&f.host, checked.preview.unwrap(), &[], NOW);
    assert_eq!(kept.items[0].status, InstallStatus::Kept);
    assert_eq!(
        fs::read_to_string(f.destination.join("my-notes.txt")).unwrap(),
        "handwritten"
    );
    assert_eq!(
        fs::read(f.project.join("skills-lock.json")).unwrap(),
        before
    );
    let checked = check_update(&f.host, f.target(), &f.skill(), &Http::new());
    let replaced = apply_install(
        &f.host,
        checked.preview.unwrap(),
        &[InstallDecision::Replace],
        NOW,
    );
    assert_eq!(replaced.items[0].status, InstallStatus::Replaced);
    assert_eq!(
        fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
        NEW
    );
    assert!(!f.destination.join("my-notes.txt").exists());
    assert!(!f.project.join(".agents/skills/renamed-upstream").exists());
    let updated = f.lock();
    assert_eq!(updated["extension"], lock["extension"]);
    assert_eq!(updated["skills"]["unrelated"], lock["skills"]["unrelated"]);
    assert_eq!(
        updated["skills"]["Original Name"]["future"],
        json!(["preserve", 7])
    );
    assert_eq!(updated["skills"]["Original Name"]["ref"], "release/stable");
    assert_eq!(updated["skills"]["Original Name"]["source"], "acme/tools");
    let removed = apply_remove(
        &f.host,
        preview_remove(&f.host, f.target(), &f.skill(), &[]).unwrap(),
    );
    assert_eq!(removed.content, RemovalStatus::Removed);
    assert_eq!(removed.source_record, RemovalStatus::Removed);
    assert!(!f.destination.exists());
    assert!(f.lock()["skills"].get("Original Name").is_none());
    assert_eq!(f.lock()["skills"]["unrelated"], lock["skills"]["unrelated"]);
}

#[test]
fn update_states_separate_no_update_deletion_failure_and_unknown_source_without_writes() {
    let f = Fixture::new();
    let original = fs::read(f.destination.join("SKILL.md")).unwrap();
    let lock_bytes = fs::read(f.project.join("skills-lock.json")).unwrap();
    let checked = check_update(&f.host, f.target(), &f.skill(), &Http::old());
    assert_eq!(checked.status, UpdateStatus::UpToDate);
    assert_eq!(checked.local_changes, LocalChanges::Unmodified);
    assert!(checked.preview.is_none());
    let mut deleted = Http::new();
    deleted.deleted = true;
    assert_eq!(
        check_update(&f.host, f.target(), &f.skill(), &deleted).status,
        UpdateStatus::DeletedUpstream
    );
    for error in [
        ApiError::NotFound,
        ApiError::Unauthorized,
        ApiError::Network("isolated offline".into()),
    ] {
        let mut http = Http::new();
        http.error = Some(error);
        assert!(matches!(
            check_update(&f.host, f.target(), &f.skill(), &http).status,
            UpdateStatus::Failed(_)
        ));
    }
    assert_eq!(fs::read(f.destination.join("SKILL.md")).unwrap(), original);
    assert_eq!(
        fs::read(f.project.join("skills-lock.json")).unwrap(),
        lock_bytes
    );
    for entry in [
        json!({"sourceType":"local","source":"../mine"}),
        json!({"sourceType":"github","source":"acme/tools","ref":"release/stable"}),
        json!({"sourceType":"github","source":"acme/tools","skillPath":"../SKILL.md"}),
        json!({"sourceType":"github","source":"acme/tools","sourceUrl":"https://github.com/acme/tools/tree/release%2Fstable/elsewhere","ref":"release/stable","skillPath":"skills/original/SKILL.md"}),
        json!({"sourceType":"github","source":"acme/tools","sourceUrl":"https://github.com/elsewhere/repo","skillPath":"skills/original/SKILL.md"}),
    ] {
        let mut lock = f.lock();
        lock["skills"]["Original Name"] = entry;
        fs::write(
            f.project.join("skills-lock.json"),
            serde_json::to_vec(&lock).unwrap(),
        )
        .unwrap();
        let before = fs::read(f.project.join("skills-lock.json")).unwrap();
        let checked = check_update(&f.host, f.target(), &f.skill(), &Http::new());
        assert!(matches!(checked.status, UpdateStatus::UnknownSource(_)));
        assert!(checked.preview.is_none());
        assert_eq!(
            fs::read(f.project.join("skills-lock.json")).unwrap(),
            before
        );
        assert_eq!(fs::read(f.destination.join("SKILL.md")).unwrap(), original);
    }
}

#[test]
fn missing_baseline_remains_unknown_and_defaults_to_keep() {
    let f = Fixture::new();
    let mut lock = f.lock();
    let entry = lock["skills"]["Original Name"].as_object_mut().unwrap();
    entry.remove("contentHash");
    entry.remove("computedHash");
    fs::write(
        f.project.join("skills-lock.json"),
        serde_json::to_vec(&lock).unwrap(),
    )
    .unwrap();
    let before = fs::read(f.project.join("skills-lock.json")).unwrap();
    let check = check_update(&f.host, f.target(), &f.skill(), &Http::new());
    assert_eq!(check.status, UpdateStatus::Available);
    assert_eq!(check.local_changes, LocalChanges::UnknownBaseline);
    assert_eq!(
        apply_install(&f.host, check.preview.unwrap(), &[], NOW).items[0].status,
        InstallStatus::Kept
    );
    assert_eq!(
        fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
        OLD
    );
    assert_eq!(
        fs::read(f.project.join("skills-lock.json")).unwrap(),
        before
    );
}

#[test]
fn cancelling_reviews_and_confirming_changed_content_or_lock_never_mutates_live_data() {
    for change_lock in [false, true] {
        let f = Fixture::new();
        let skill = f.skill();
        let preview = preview_remove(&f.host, f.target(), &skill, &[]).unwrap();
        drop(preview);
        assert_eq!(
            fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
            OLD
        );
        let update = check_update(&f.host, f.target(), &skill, &Http::new());
        drop(update);
        let remove = preview_remove(&f.host, f.target(), &skill, &[]).unwrap();
        let update = check_update(&f.host, f.target(), &skill, &Http::new());
        if change_lock {
            let mut lock = f.lock();
            lock["afterConfirmation"] = json!("keep");
            fs::write(
                f.project.join("skills-lock.json"),
                serde_json::to_vec(&lock).unwrap(),
            )
            .unwrap();
        } else {
            fs::write(f.destination.join("SKILL.md"), "edited after review").unwrap();
        }
        let content = fs::read(f.destination.join("SKILL.md")).unwrap();
        let lock = fs::read(f.project.join("skills-lock.json")).unwrap();
        assert!(matches!(
            apply_remove(&f.host, remove).content,
            RemovalStatus::Failed(_)
        ));
        assert!(matches!(
            apply_install(
                &f.host,
                update.preview.unwrap(),
                &[InstallDecision::Replace],
                NOW
            )
            .items[0]
                .status,
            InstallStatus::Failed(_)
        ));
        assert_eq!(fs::read(f.destination.join("SKILL.md")).unwrap(), content);
        assert_eq!(fs::read(f.project.join("skills-lock.json")).unwrap(), lock);
    }
}

#[test]
fn malformed_or_unsupported_locks_block_both_maintenance_writes_without_normalizing() {
    for bytes in [
        "{bad",
        "{\"version\":9,\"skills\":{}}",
        "{\"version\":1,\"skills\":[]}",
        "{\"version\":1,\"skills\":{\"bad\":null}}",
    ] {
        let f = Fixture::new();
        let skill = f.skill();
        fs::write(f.project.join("skills-lock.json"), bytes).unwrap();
        assert!(matches!(
            check_update(&f.host, f.target(), &skill, &Http::new()).status,
            UpdateStatus::Failed(_)
        ));
        assert!(preview_remove(&f.host, f.target(), &skill, &[]).is_err());
        assert_eq!(
            fs::read_to_string(f.project.join("skills-lock.json")).unwrap(),
            bytes
        );
        assert_eq!(
            fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
            OLD
        );
    }
}

#[test]
fn source_search_finds_installation_by_repository_url_and_recorded_path() {
    let f = Fixture::new();
    let mut lock = f.lock();
    lock["skills"]["Original Name"]["sourceUrl"] = json!("https://github.com/acme/tools.git");
    fs::write(
        f.project.join("skills-lock.json"),
        serde_json::to_vec(&lock).unwrap(),
    )
    .unwrap();
    let snapshot = scan(
        &f.host,
        &ScanRequest::new(&f.host.home).with_project(&f.project),
    )
    .unwrap();
    for query in [
        "acme/tools",
        "https://github.com/acme/tools.git",
        "skills/original/SKILL.md",
    ] {
        let found = snapshot.filtered(query, None).next().unwrap();
        assert_eq!(found.path(), f.host.canonicalize(&f.destination).unwrap());
    }
}

#[cfg(unix)]
fn link(target: &Path, path: &Path) {
    std::os::unix::fs::symlink(target, path).unwrap();
}
#[cfg(windows)]
fn link(target: &Path, path: &Path) {
    // cmd's mklink builtin parses forward slashes as switches, unlike Win32 FS APIs.
    let mut command = std::process::Command::new("cmd.exe");
    command
        .args(["/D", "/C", "mklink", "/J"])
        .arg(path.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"));
    lyrel_core::core::proc::hide_console(&mut command);
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "junction creation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn removal_finds_alias_targets_and_keeps_real_copies_other_targets_and_same_names() {
    let f = Fixture::new();
    let private = f.host.home.join("private/skills");
    fs::create_dir_all(&private).unwrap();
    let nested = private.join("group");
    fs::create_dir(&nested).unwrap();
    let alias = private.join("different-alias");
    link(&f.destination, &alias);
    let second = private.join("second-alias");
    link(&f.destination, &second);
    let nested_alias = nested.join("also-not-the-name");
    link(&f.destination, &nested_alias);
    let copy = private.join("original-name");
    fs::create_dir(&copy).unwrap();
    fs::write(copy.join("SKILL.md"), "private real copy").unwrap();
    let elsewhere = f.host.home.join("unrelated");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("SKILL.md"), "another target").unwrap();
    let other_link = private.join("other-target");
    link(&elsewhere, &other_link);
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[private.clone()]).unwrap();
    for expected in [&alias, &second, &nested_alias] {
        assert!(preview.links().iter().any(|link| link.path() == expected));
    }
    assert!(
        !preview
            .links()
            .iter()
            .any(|link| link.path() == copy || link.path() == other_link)
    );
    drop(preview);
    assert_eq!(fs::read_to_string(alias.join("SKILL.md")).unwrap(), OLD);
    let report = apply_remove(
        &f.host,
        preview_remove(&f.host, f.target(), &f.skill(), &[private]).unwrap(),
    );
    assert_eq!(report.content, RemovalStatus::Removed);
    for expected in [&alias, &second, &nested_alias] {
        assert_eq!(
            report
                .links
                .iter()
                .find(|result| result.path == *expected)
                .unwrap()
                .status,
            RemovalStatus::Removed
        );
        assert!(fs::symlink_metadata(expected).is_err());
    }
    assert_eq!(
        fs::read_to_string(copy.join("SKILL.md")).unwrap(),
        "private real copy"
    );
    assert_eq!(
        fs::read_to_string(other_link.join("SKILL.md")).unwrap(),
        "another target"
    );
    assert_eq!(
        fs::read_to_string(elsewhere.join("SKILL.md")).unwrap(),
        "another target"
    );
    f.host.inner.remove(&other_link, false).unwrap();
}

#[cfg(unix)]
#[test]
fn relative_link_target_is_resolved_on_selected_host_and_unlinked_without_following() {
    let f = Fixture::new();
    let root = f.project.join(".pi/skills");
    fs::create_dir_all(&root).unwrap();
    let alias = root.join("relative-alias");
    link(Path::new("../../.agents/skills/original-name"), &alias);
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[root]).unwrap();
    assert_eq!(
        preview.links()[0].target(),
        Path::new("../../.agents/skills/original-name")
    );
    assert_eq!(
        apply_remove(&f.host, preview).links[0].status,
        RemovalStatus::Removed
    );
    assert!(fs::symlink_metadata(alias).is_err());
}

#[test]
fn link_retargeted_after_confirmation_is_preserved_with_its_new_target() {
    let f = Fixture::new();
    let root = f.host.home.join("private");
    fs::create_dir(&root).unwrap();
    let alias = root.join("alias");
    link(&f.destination, &alias);
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[root]).unwrap();
    f.host.inner.remove(&alias, false).unwrap();
    let other = f.host.home.join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("SKILL.md"), "new target").unwrap();
    link(&other, &alias);
    let report = apply_remove(&f.host, preview);
    assert_eq!(report.content, RemovalStatus::Removed);
    assert!(matches!(report.links[0].status, RemovalStatus::Retained(_)));
    assert_eq!(
        fs::read_to_string(alias.join("SKILL.md")).unwrap(),
        "new target"
    );
    f.host.inner.remove(&alias, false).unwrap();
}

#[test]
fn unavailable_link_capability_reports_retained_alias_instead_of_claiming_no_links() {
    let mut f = Fixture::new();
    let root = f.host.home.join("private");
    fs::create_dir(&root).unwrap();
    let alias = root.join("alias");
    link(&f.destination, &alias);
    f.host.unsupported_links = true;
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[root]).unwrap();
    assert!(
        preview
            .warnings()
            .iter()
            .any(|warning| warning.contains("upgrade read-link") && warning.contains("retained"))
    );
    let report = apply_remove(&f.host, preview);
    assert_eq!(report.content, RemovalStatus::Removed);
    assert!(fs::symlink_metadata(&alias).is_ok());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("upgrade read-link"))
    );
    f.host.inner.remove(&alias, false).unwrap();
}

#[test]
fn lock_publication_denial_restores_original_installation_and_exact_source_bytes() {
    let mut f = Fixture::new();
    let skill = f.skill();
    let before = fs::read(f.project.join("skills-lock.json")).unwrap();
    let preview = preview_remove(&f.host, f.target(), &skill, &[]).unwrap();
    f.host.deny_lock = true;
    let report = apply_remove(&f.host, preview);
    assert!(matches!(&report.content,RemovalStatus::Failed(message) if message.contains("denied")));
    assert_eq!(
        fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
        OLD
    );
    assert_eq!(
        fs::read(f.project.join("skills-lock.json")).unwrap(),
        before
    );
}

#[test]
fn denied_content_cleanup_reports_detached_recoverable_content_and_committed_record_removal() {
    let mut f = Fixture::new();
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[]).unwrap();
    f.host.deny_remove = Some(f.project.join(".agents/skills"));
    let report = apply_remove(&f.host, preview);
    assert!(
        matches!(&report.content,RemovalStatus::Failed(message) if message.contains("installation detached") && message.contains(".lyrel-remove-"))
    );
    assert_eq!(report.source_record, RemovalStatus::Removed);
    assert!(!f.destination.exists());
    let detached = fs::read_dir(f.project.join(".agents/skills"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".lyrel-remove-")
        })
        .unwrap();
    assert_eq!(fs::read_to_string(detached.join("SKILL.md")).unwrap(), OLD);
    assert!(f.lock()["skills"].get("Original Name").is_none());
}

#[test]
fn lost_publication_reply_reports_unknown_and_preserves_recovery_candidates_without_repeating() {
    let mut f = Fixture::new();
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[]).unwrap();
    let original_lock = fs::read(f.project.join("skills-lock.json")).unwrap();
    f.host.lose_remove_reply = true;
    let report = apply_remove(&f.host, preview);
    assert!(
        matches!(&report.content,RemovalStatus::OutcomeUnknown(message) if message.contains("do not blindly repeat removal"))
    );
    assert!(!f.destination.exists());
    assert_eq!(
        fs::read(f.project.join("skills-lock.json")).unwrap(),
        original_lock
    );
    let detached = fs::read_dir(f.project.join(".agents/skills"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".lyrel-remove-")
        })
        .unwrap();
    assert_eq!(fs::read_to_string(detached.join("SKILL.md")).unwrap(), OLD);
    assert!(f.project.join(".lyrel-skills-write.lock").exists());
}

#[test]
fn confirmation_cannot_be_redirected_to_another_host() {
    let mut f = Fixture::new();
    let skill = f.skill();
    let remove = preview_remove(&f.host, f.target(), &skill, &[]).unwrap();
    let update = check_update(&f.host, f.target(), &skill, &Http::new());
    let bytes = fs::read(f.project.join("skills-lock.json")).unwrap();
    f.host.other_host = true;
    assert!(matches!(
        apply_remove(&f.host, remove).content,
        RemovalStatus::Failed(_)
    ));
    assert!(matches!(
        apply_install(
            &f.host,
            update.preview.unwrap(),
            &[InstallDecision::Replace],
            NOW
        )
        .items[0]
            .status,
        InstallStatus::Failed(_)
    ));
    assert_eq!(
        fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
        OLD
    );
    assert_eq!(fs::read(f.project.join("skills-lock.json")).unwrap(), bytes);
}

#[test]
fn alias_retargeted_during_publication_is_rechecked_before_unlink() {
    let mut f = Fixture::new();
    let root = f.host.home.join("private");
    fs::create_dir(&root).unwrap();
    let alias = root.join("alias");
    link(&f.destination, &alias);
    let other = f.host.home.join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("SKILL.md"), "keep new target").unwrap();
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[root]).unwrap();
    f.host.retarget_on_detach = Some((alias.clone(), other));
    let report = apply_remove(&f.host, preview);
    assert_eq!(report.content, RemovalStatus::Removed);
    assert!(matches!(report.links[0].status, RemovalStatus::Retained(_)));
    assert_eq!(
        fs::read_to_string(alias.join("SKILL.md")).unwrap(),
        "keep new target"
    );
    f.host.inner.remove(&alias, false).unwrap();
}

#[test]
fn global_update_preserves_recorded_source_url_ref_install_time_and_unknown_fields() {
    let f = Fixture::new();
    let target = InstallTarget::user(
        &f.host,
        f.host.home.clone(),
        Some(f.host.home.join(".state")),
        f.project.clone(),
    )
    .unwrap();
    let source = GitHubSource::parse("acme/tools#release%2Fstable").unwrap();
    let catalog = discover(&Http::old(), &source).unwrap();
    let preview = preview_install(
        &f.host,
        target.clone(),
        vec![catalog.prepare(&Http::old(), 0).unwrap()],
    )
    .unwrap();
    assert_eq!(
        apply_install(&f.host, preview, &[InstallDecision::Install], NOW).items[0].status,
        InstallStatus::Installed
    );
    let path = f
        .host
        .canonicalize(&f.host.home.join(".agents/skills/original-name"))
        .unwrap();
    let snapshot = scan(
        &f.host,
        &ScanRequest::new(&f.host.home).with_project(&f.project),
    )
    .unwrap();
    let skill = snapshot
        .installations()
        .iter()
        .find(|skill| skill.path() == path)
        .unwrap();
    let mut lock: Value = serde_json::from_slice(&fs::read(target.lock_path()).unwrap()).unwrap();
    let source_url = "https://github.com/acme/tools/tree/release%2Fstable/skills/original";
    lock["skills"]["Original Name"]["sourceUrl"] = json!(source_url);
    lock["skills"]["Original Name"]["installedAt"] = json!("2026-01-01T00:00:00.000Z");
    lock["skills"]["Original Name"]["future"] = json!({"policy":true});
    lock["dismissed"] = json!({"findSkillsPrompt":true});
    fs::write(target.lock_path(), serde_json::to_vec(&lock).unwrap()).unwrap();
    let check = check_update(&f.host, target.clone(), skill, &Http::new());
    assert_eq!(check.status, UpdateStatus::Available);
    assert_eq!(
        apply_install(
            &f.host,
            check.preview.unwrap(),
            &[InstallDecision::Replace],
            NOW
        )
        .items[0]
            .status,
        InstallStatus::Replaced
    );
    let updated: Value = serde_json::from_slice(&fs::read(target.lock_path()).unwrap()).unwrap();
    assert_eq!(updated["skills"]["Original Name"]["sourceUrl"], source_url);
    assert_eq!(
        updated["skills"]["Original Name"]["installedAt"],
        "2026-01-01T00:00:00.000Z"
    );
    assert_eq!(updated["skills"]["Original Name"]["ref"], "release/stable");
    assert_eq!(
        updated["skills"]["Original Name"]["future"],
        json!({"policy":true})
    );
    assert_eq!(updated["dismissed"], json!({"findSkillsPrompt":true}));
    assert_eq!(fs::read_to_string(path.join("SKILL.md")).unwrap(), NEW);
    assert_eq!(
        fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
        OLD
    );
}

#[test]
fn private_root_discovery_reuses_selected_omp_profile_without_writing_configuration() {
    use lyrel_core::skills::maintenance::private_roots;
    let f = Fixture::new();
    let request = ScanRequest::new(&f.host.home)
        .with_project(&f.project)
        .with_launch_commands(std::collections::HashMap::from([(
            "omp".into(),
            "OMP_PROFILE=research omp".into(),
        )]));
    let roots = private_roots(&f.host, &request).unwrap();
    assert!(roots.contains(&f.host.home.join(".omp/profiles/research/agent/skills")));
    assert!(!roots.contains(&f.host.home.join(".omp/agent/skills")));
    assert!(
        !f.host
            .home
            .join(".omp/profiles/research/agent/config.yml")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn replaced_installation_path_is_never_followed_during_removal_or_update() {
    let f = Fixture::new();
    let skill = f.skill();
    let remove = preview_remove(&f.host, f.target(), &skill, &[]).unwrap();
    let update = check_update(&f.host, f.target(), &skill, &Http::new());
    let original = f.destination.with_extension("original");
    fs::rename(&f.destination, &original).unwrap();
    let external = f.host.home.join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("SKILL.md"), "do not touch").unwrap();
    link(&external, &f.destination);
    assert!(matches!(
        apply_remove(&f.host, remove).content,
        RemovalStatus::Failed(_)
    ));
    assert!(matches!(
        apply_install(
            &f.host,
            update.preview.unwrap(),
            &[InstallDecision::Replace],
            NOW
        )
        .items[0]
            .status,
        InstallStatus::Failed(_)
    ));
    assert_eq!(
        fs::read_to_string(external.join("SKILL.md")).unwrap(),
        "do not touch"
    );
    assert_eq!(fs::read_to_string(original.join("SKILL.md")).unwrap(), OLD);
}

#[cfg(unix)]
#[test]
#[ignore = "requires an unprivileged Unix process; run explicitly for kernel permission-denial evidence"]
fn kernel_permission_denial_keeps_installation_and_source_lock_exactly() {
    use std::os::unix::fs::PermissionsExt;
    let uid = std::process::Command::new("id").arg("-u").output().unwrap();
    assert_ne!(
        String::from_utf8_lossy(&uid.stdout).trim(),
        "0",
        "permission-denial test must not run as root"
    );
    let f = Fixture::new();
    let root = f.project.join(".agents/skills");
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[]).unwrap();
    let before = fs::read(f.project.join("skills-lock.json")).unwrap();
    let permissions = fs::metadata(&root).unwrap().permissions();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
    let report = apply_remove(&f.host, preview);
    fs::set_permissions(&root, permissions).unwrap();
    assert!(matches!(report.content, RemovalStatus::Failed(_)));
    assert_eq!(
        fs::read_to_string(f.destination.join("SKILL.md")).unwrap(),
        OLD
    );
    assert_eq!(
        fs::read(f.project.join("skills-lock.json")).unwrap(),
        before
    );
}

#[test]
fn untracked_files_excluded_by_cli_hashing_still_mark_the_installation_modified() {
    let f = Fixture::new();
    fs::create_dir(f.destination.join("node_modules")).unwrap();
    fs::write(
        f.destination.join("node_modules/handwritten.txt"),
        "keep local addition",
    )
    .unwrap();
    let checked = check_update(&f.host, f.target(), &f.skill(), &Http::new());
    assert_eq!(checked.status, UpdateStatus::Available);
    assert_eq!(checked.local_changes, LocalChanges::Modified);
    assert_eq!(
        apply_install(&f.host, checked.preview.unwrap(), &[], NOW).items[0].status,
        InstallStatus::Kept
    );
    assert_eq!(
        fs::read_to_string(f.destination.join("node_modules/handwritten.txt")).unwrap(),
        "keep local addition"
    );
}

#[test]
fn removal_of_untracked_installation_leaves_source_lock_bytes_and_other_items_untouched() {
    let f = Fixture::new();
    let mut lock = f.lock();
    lock["skills"]
        .as_object_mut()
        .unwrap()
        .remove("Original Name");
    lock["skills"]["other"] = json!({"sourceType":"local","source":"../other","future":7});
    let bytes = serde_json::to_vec(&lock).unwrap();
    fs::write(f.project.join("skills-lock.json"), &bytes).unwrap();
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[]).unwrap();
    assert!(preview.lock_key().is_none());
    let report = apply_remove(&f.host, preview);
    assert_eq!(report.content, RemovalStatus::Removed);
    assert!(matches!(report.source_record, RemovalStatus::Retained(_)));
    assert_eq!(fs::read(f.project.join("skills-lock.json")).unwrap(), bytes);
}

#[test]
fn replaced_private_parent_with_identical_link_text_is_preserved() {
    let f = Fixture::new();
    let root = f.host.home.join("private");
    fs::create_dir(&root).unwrap();
    let alias = root.join("alias");
    link(&f.destination, &alias);
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[root.clone()]).unwrap();
    let previous = root.with_extension("previous");
    fs::rename(&root, &previous).unwrap();
    fs::create_dir(&root).unwrap();
    link(&f.destination, &alias);
    let permissions = fs::metadata(&root).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&root, readonly).unwrap();
    let report = apply_remove(&f.host, preview);
    fs::set_permissions(&root, permissions).unwrap();
    assert_eq!(report.content, RemovalStatus::Removed);
    assert!(matches!(report.links[0].status, RemovalStatus::Retained(_)));
    assert!(fs::symlink_metadata(&alias).is_ok());
    f.host.inner.remove(&alias, false).unwrap();
    f.host.inner.remove(&previous.join("alias"), false).unwrap();
}

#[test]
fn denied_link_cleanup_reports_its_exact_remaining_path_without_repeating_removal() {
    let mut f = Fixture::new();
    let root = f.host.home.join("private");
    fs::create_dir(&root).unwrap();
    let alias = root.join("alias");
    link(&f.destination, &alias);
    let preview = preview_remove(&f.host, f.target(), &f.skill(), &[root]).unwrap();
    f.host.deny_remove = Some(alias.clone());
    let report = apply_remove(&f.host, preview);
    assert_eq!(report.content, RemovalStatus::Removed);
    assert_eq!(report.source_record, RemovalStatus::Removed);
    assert_eq!(report.links[0].path, alias);
    assert!(
        matches!(&report.links[0].status,RemovalStatus::Failed(message) if message.contains("do not repeat the entire removal"))
    );
    assert!(fs::symlink_metadata(&alias).is_ok());
    assert!(!f.destination.exists());
    f.host.inner.remove(&alias, false).unwrap();
}
