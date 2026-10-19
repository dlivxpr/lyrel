#![cfg(feature = "github")]

use lyrel_core::{
    host::{Host, local::LocalHost},
    skills::{
        catalog::{GitHubSource, PreparedSkill, SkillCandidate, SkillFile},
        install::{
            ExistingContent, InstallDecision, InstallStatus, InstallTarget, apply_install,
            preview_install,
        },
    },
};
use std::{fs, path::Path};

const NOW: &str = "2026-10-19T08:00:00.000Z";
fn prepared(name: &str) -> PreparedSkill {
    PreparedSkill::new(
        SkillCandidate::new(
            name.into(),
            "Review source".into(),
            GitHubSource::parse("example/skills#main").unwrap(),
            format!("skills/{name}/SKILL.md"),
            "a".repeat(40),
        ),
        vec![SkillFile::new(
            "SKILL.md".into(),
            format!("---\nname: {name}\ndescription: Review source\n---\n# Review\n").into_bytes(),
        )],
        "b".repeat(64),
    )
}
fn project(root: &Path) -> InstallTarget {
    fs::create_dir_all(root).unwrap();
    InstallTarget::project(&*LocalHost::new(), root.to_path_buf()).unwrap()
}

#[test]
fn reviewing_and_cancelling_installation_creates_no_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let host = LocalHost::new();
    let preview = preview_install(&*host, project(&root), vec![prepared("Review")]).unwrap();
    assert_eq!(
        preview.items()[0].destination(),
        root.join(".agents/skills/review")
    );
    assert_eq!(preview.items()[0].existing(), &ExistingContent::Absent);
    assert_eq!(preview.target().lock_path(), root.join("skills-lock.json"));
    drop(preview);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn confirmed_project_install_writes_shared_content_and_cli_v1_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let host = LocalHost::new();
    let preview = preview_install(&*host, project(&root), vec![prepared("review")]).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    assert!(
        fs::read_to_string(root.join(".agents/skills/review/SKILL.md"))
            .unwrap()
            .contains("# Review")
    );
    let bytes = fs::read(root.join("skills-lock.json")).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    let lock: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(lock["version"], 1);
    assert_eq!(lock["skills"]["review"]["source"], "example/skills");
    assert_eq!(lock["skills"]["review"]["computedHash"], "b".repeat(64));
    assert!(lock["skills"]["review"].get("sourceUrl").is_none());
    assert!(!root.join(".pi").exists());
    assert!(!root.join(".claude").exists());
}

#[test]
fn existing_content_is_kept_by_default_and_only_explicit_replace_overwrites() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let destination = root.join(".agents/skills/review");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("local.txt"), "my changes").unwrap();
    let host = LocalHost::new();
    let preview = preview_install(&*host, target.clone(), vec![prepared("review")]).unwrap();
    assert_eq!(
        preview.items()[0].existing(),
        &ExistingContent::Directory {
            files: 1,
            bytes: 10
        }
    );
    let report = apply_install(&*host, preview, &[], NOW);
    assert_eq!(report.items[0].status, InstallStatus::Kept);
    assert_eq!(
        fs::read_to_string(destination.join("local.txt")).unwrap(),
        "my changes"
    );
    assert!(!root.join("skills-lock.json").exists());
    let preview = preview_install(&*host, target.clone(), vec![prepared("review")]).unwrap();
    let refused = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert!(matches!(refused.items[0].status, InstallStatus::Failed(_)));
    assert!(destination.join("local.txt").exists());
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    let replaced = apply_install(&*host, preview, &[InstallDecision::Replace], NOW);
    assert_eq!(replaced.items[0].status, InstallStatus::Replaced);
    assert!(!destination.join("local.txt").exists());
    assert!(destination.join("SKILL.md").exists());
}

#[test]
fn damaged_or_unknown_locks_are_never_rewritten() {
    for bytes in [
        "{bad",
        "{\"version\":4,\"skills\":{}}",
        "{\"version\":0,\"skills\":{}}",
        "{\"version\":1,\"skills\":[]}",
        "{\"version\":1,\"skills\":{\"old\":null}}",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let target = project(&root);
        fs::write(root.join("skills-lock.json"), bytes).unwrap();
        let host = LocalHost::new();
        assert!(preview_install(&*host, target, vec![prepared("review")]).is_err());
        assert_eq!(
            fs::read_to_string(root.join("skills-lock.json")).unwrap(),
            bytes
        );
        assert!(!root.join(".agents").exists());
    }
}

#[test]
fn project_lock_preserves_top_level_extensions_changed_entry_extensions_and_unrelated_entries() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let original = serde_json::json!({"version":1,"extension":{"policy":true},"skills":{
        "review":{"source":"old/repo","sourceType":"github","computedHash":"old","subagents":["eve"],"future":{"keep":7}},
        "untouched":{"sourceType":"local","source":"../private","opaque":[1,null,"x"]}
    }});
    fs::write(
        root.join("skills-lock.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    let host = LocalHost::new();
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    let updated: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("skills-lock.json")).unwrap()).unwrap();
    assert_eq!(updated["extension"], original["extension"]);
    assert_eq!(
        updated["skills"]["untouched"],
        original["skills"]["untouched"]
    );
    assert_eq!(
        updated["skills"]["review"]["subagents"],
        original["skills"]["review"]["subagents"]
    );
    assert_eq!(
        updated["skills"]["review"]["future"],
        original["skills"]["review"]["future"]
    );
    assert_eq!(updated["skills"]["review"]["source"], "example/skills");
}

#[test]
fn user_lock_uses_cli_v3_xdg_location_and_preserves_install_time() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(cwd.join("relative-state/skills")).unwrap();
    let lock_path = cwd.join("relative-state/skills/.skill-lock.json");
    fs::write(&lock_path, r#"{"version":3,"dismissed":{"findSkillsPrompt":true},"unknown":8,"skills":{"review":{"installedAt":"2026-01-01T00:00:00.000Z","future":42}}}"#).unwrap();
    let host = LocalHost::new();
    let target =
        InstallTarget::user(&*host, home.clone(), Some("relative-state".into()), cwd).unwrap();
    assert_eq!(target.lock_path(), lock_path);
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    let bytes = fs::read(&lock_path).unwrap();
    assert_ne!(bytes.last(), Some(&b'\n'));
    let lock: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(lock["version"], 3);
    assert_eq!(
        lock["skills"]["review"]["sourceUrl"],
        "https://github.com/example/skills.git"
    );
    assert_eq!(lock["skills"]["review"]["skillFolderHash"], "a".repeat(40));
    assert_eq!(
        lock["skills"]["review"]["installedAt"],
        "2026-01-01T00:00:00.000Z"
    );
    assert_eq!(lock["skills"]["review"]["updatedAt"], NOW);
    assert_eq!(lock["skills"]["review"]["future"], 42);
    assert_eq!(lock["dismissed"]["findSkillsPrompt"], true);
    assert_eq!(lock["unknown"], 8);
    assert!(home.join(".agents/skills/review/SKILL.md").exists());
    assert!(!home.join(".agents/.skill-lock.json").exists());
}

#[test]
fn confirming_one_item_does_not_install_another_and_empty_xdg_uses_home_lock() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let host = LocalHost::new();
    let target = InstallTarget::user(&*host, home.clone(), Some("".into()), home.clone()).unwrap();
    assert_eq!(target.lock_path(), home.join(".agents/.skill-lock.json"));
    let preview =
        preview_install(&*host, target, vec![prepared("first"), prepared("second")]).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    assert_eq!(report.items[1].status, InstallStatus::Kept);
    assert!(!home.join(".agents/skills/second").exists());
}

#[test]
fn confirmation_does_not_authorize_changed_content_or_changed_lock() {
    for change_lock in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let target = project(&root);
        let destination = root.join(".agents/skills/review");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("local.txt"), "original").unwrap();
        let lock = root.join("skills-lock.json");
        fs::write(&lock, r#"{"version":1,"skills":{}}"#).unwrap();
        let host = LocalHost::new();
        let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
        if change_lock {
            fs::write(&lock, r#"{"version":1,"skills":{},"new":true}"#).unwrap();
        } else {
            fs::write(
                destination.join("local.txt"),
                "user changed this after preview",
            )
            .unwrap();
        }
        let before_lock = fs::read(&lock).unwrap();
        let before_content = fs::read(destination.join("local.txt")).unwrap();
        let report = apply_install(&*host, preview, &[InstallDecision::Replace], NOW);
        assert!(matches!(report.items[0].status, InstallStatus::Failed(_)));
        assert_eq!(fs::read(&lock).unwrap(), before_lock);
        assert_eq!(
            fs::read(destination.join("local.txt")).unwrap(),
            before_content
        );
        assert!(!destination.join("SKILL.md").exists());
    }
}

#[test]
fn prepared_path_escape_and_same_target_collisions_fail_before_writing() {
    for path in [
        "../escape",
        "/escape",
        "C:/escape",
        "nested/../../escape",
        "nested\\escape",
        "nested/file:stream",
        "NUL",
        "nested/CON.txt",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let target = project(&root);
        let mut skill = prepared("review");
        skill
            .files
            .push(SkillFile::new(path.into(), b"malicious".to_vec()));
        let host = LocalHost::new();
        assert!(
            preview_install(&*host, target, vec![skill]).is_err(),
            "{path}"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let host = LocalHost::new();
    assert!(
        preview_install(
            &*host,
            project(&root),
            vec![prepared("Review!"), prepared("review")]
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

#[test]
fn user_added_empty_nested_directory_invalidates_replace_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let directory = root.join(".agents/skills/review");
    fs::create_dir_all(directory.join("references")).unwrap();
    let host = LocalHost::new();
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    fs::create_dir(directory.join("references/new-empty")).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Replace], NOW);
    assert!(matches!(report.items[0].status, InstallStatus::Failed(_)));
    assert!(directory.join("references/new-empty").exists());
    assert!(!directory.join("SKILL.md").exists());
}

#[cfg(unix)]
fn directory_link(target: &Path, path: &Path) {
    std::os::unix::fs::symlink(target, path).unwrap();
}
#[cfg(windows)]
fn directory_link(target: &Path, path: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(path.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated junction creation: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(any(unix, windows))]
#[test]
fn linked_ancestor_linked_installation_and_post_confirmation_redirect_are_rejected() {
    for link_ancestor in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let target = project(&root);
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "untouched").unwrap();
        if link_ancestor {
            directory_link(&outside, &root.join(".agents"));
        } else {
            fs::create_dir_all(root.join(".agents/skills")).unwrap();
            directory_link(&outside, &root.join(".agents/skills/review"));
        }
        let host = LocalHost::new();
        assert!(preview_install(&*host, target, vec![prepared("review")]).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "untouched"
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        assert!(!root.join("skills-lock.json").exists());
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let host = LocalHost::new();
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    directory_link(&outside, &root.join(".agents"));
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert!(matches!(report.items[0].status, InstallStatus::Failed(_)));
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn dangling_lock_and_destination_links_are_not_mistaken_for_missing_paths() {
    for lock_link in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let target = project(&root);
        fs::create_dir_all(root.join(".agents/skills")).unwrap();
        let path = if lock_link {
            root.join("skills-lock.json")
        } else {
            root.join(".agents/skills/review")
        };
        std::os::unix::fs::symlink(temp.path().join("absent"), &path).unwrap();
        let host = LocalHost::new();
        assert!(preview_install(&*host, target, vec![prepared("review")]).is_err());
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }
}

// A real filesystem Host with only the external write operation faulted. Reads,
// directory identity, staging and rollback all exercise LocalHost in a sandbox.
struct WriteFailure {
    inner: lyrel_core::host::SharedHost,
    fail_lock_commit: bool,
    fail_name: Option<String>,
    change_target_after_stage: Option<std::path::PathBuf>,
    fail_restore_lock: bool,
}
impl Host for WriteFailure {
    fn id(&self) -> lyrel_core::host::HostId {
        self.inner.id()
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(
        &self,
        p: &Path,
        root: Option<&Path>,
    ) -> std::io::Result<Vec<lyrel_core::host::Entry>> {
        self.inner.read_dir(p, root)
    }
    fn stat(&self, p: &Path) -> std::io::Result<lyrel_core::host::Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> std::io::Result<std::path::PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[std::path::PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        hidden: bool,
    ) -> std::io::Result<Vec<lyrel_core::host::SearchHit>> {
        self.inner.search(roots, query, limit, max_dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[std::path::PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> std::io::Result<lyrel_core::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> std::io::Result<lyrel_core::host::Meta> {
        if self
            .fail_name
            .as_ref()
            .is_some_and(|name| p.file_name().is_some_and(|file| file == name.as_str()))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "isolated write denied",
            ));
        }
        let written = self.inner.write_file(p, bytes)?;
        if !self.fail_lock_commit && p.file_name().is_some_and(|name| name == "SKILL.md") {
            if let Some(target) = &self.change_target_after_stage {
                fs::write(target, "user edited while staging")?;
            }
        }
        Ok(written)
    }
    fn create_file_new(&self, p: &Path) -> std::io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        if self.fail_lock_commit
            && from.extension().is_some_and(|ext| ext == "stage")
            && to
                .file_name()
                .is_some_and(|file| file == "skills-lock.json")
        {
            if let Some(target) = &self.change_target_after_stage {
                fs::write(target, "user edited during lock commit")?;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "isolated lock commit denied",
            ));
        }
        if self.fail_restore_lock && from.extension().is_some_and(|ext| ext == "backup") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "isolated lock restore denied",
            ));
        }
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, p: &Path, args: &[&str]) -> std::io::Result<lyrel_core::host::Output> {
        self.inner.git(p, args)
    }
    fn shells(&self) -> std::io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, p: &[std::path::PathBuf]) -> std::io::Result<lyrel_core::host::WatchSub> {
        self.inner.watch(p)
    }
}

#[test]
fn stage_write_failure_is_not_success_and_other_confirmed_items_can_succeed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let host = WriteFailure {
        inner: LocalHost::new(),
        fail_lock_commit: false,
        fail_name: Some("denied.txt".into()),
        change_target_after_stage: None,
        fail_restore_lock: false,
    };
    let mut first = prepared("first");
    first
        .files
        .push(SkillFile::new("denied.txt".into(), b"fail".to_vec()));
    let preview = preview_install(&host, target, vec![first, prepared("second")]).unwrap();
    let report = apply_install(
        &host,
        preview,
        &[InstallDecision::Install, InstallDecision::Install],
        NOW,
    );
    assert!(
        matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains("isolated write denied"))
    );
    assert_eq!(report.items[1].status, InstallStatus::Installed);
    assert!(!root.join(".agents/skills/first").exists());
    assert!(root.join(".agents/skills/second/SKILL.md").exists());
    let lock: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("skills-lock.json")).unwrap()).unwrap();
    assert!(lock["skills"].get("first").is_none());
    assert_eq!(lock["skills"]["second"]["source"], "example/skills");
    assert_eq!(
        fs::read_dir(root.join(".agents/skills")).unwrap().count(),
        1
    );
}

#[test]
fn source_lock_commit_failure_restores_original_directory_and_exact_lock_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    fs::create_dir_all(root.join(".agents/skills/review")).unwrap();
    fs::write(root.join(".agents/skills/review/local.txt"), "keep my work").unwrap();
    let lock_bytes = b"{\"version\":1,\"skills\":{},\"extension\":true}\n";
    fs::write(root.join("skills-lock.json"), lock_bytes).unwrap();
    let host = WriteFailure {
        inner: LocalHost::new(),
        fail_lock_commit: true,
        fail_name: None,
        change_target_after_stage: None,
        fail_restore_lock: false,
    };
    let preview = preview_install(&host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&host, preview, &[InstallDecision::Replace], NOW);
    assert!(
        matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains("isolated lock commit denied"))
    );
    assert_eq!(fs::read(root.join("skills-lock.json")).unwrap(), lock_bytes);
    assert_eq!(
        fs::read_to_string(root.join(".agents/skills/review/local.txt")).unwrap(),
        "keep my work"
    );
    assert!(!root.join(".agents/skills/review/SKILL.md").exists());
    assert_eq!(
        fs::read_dir(root.join(".agents/skills")).unwrap().count(),
        1
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
}

#[test]
fn newly_installed_content_and_cli_origin_are_immediately_browsable() {
    use lyrel_core::skills::{ScanRequest, scan};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("project");
    fs::create_dir(&home).unwrap();
    let target = project(&root);
    let host = LocalHost::new();
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    let launches = ["omp", "pi", "kimi", "opencode", "hermes"].into_iter().map(|agent| {
        let h = home.to_string_lossy();
        (agent.to_string(), format!("HOME='{h}' USERPROFILE='{h}' PROGRAMDATA='{h}/programdata' CLAUDE_CONFIG_DIR='{h}/.claude' XDG_DATA_HOME='{h}/.data' XDG_STATE_HOME='{h}/.state' PI_CONFIG_DIR=.omp OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{h}/.pi/agent' KIMI_CODE_HOME='{h}/.kimi-code' HERMES_HOME='{h}/.hermes' XDG_CONFIG_HOME='{h}/.config' OPENCODE_CONFIG_DIR='{h}/.config/opencode' {agent}"))
    }).collect();
    let snapshot = scan(
        &*host,
        &ScanRequest::new(&home)
            .with_project(&root)
            .with_launch_commands(launches),
    )
    .unwrap();
    let installation = snapshot
        .installations()
        .iter()
        .find(|skill| skill.name() == "review")
        .unwrap();
    assert!(installation.markdown().unwrap().contains("# Review"));
    let record = installation.source_record().unwrap();
    assert_eq!(record.source(), "example/skills");
    assert_eq!(record.reference(), Some("main"));
    assert_eq!(record.skill_path(), Some("skills/review/SKILL.md"));
    assert_eq!(record.lock_path(), root.join("skills-lock.json"));
}

#[test]
fn edit_during_staging_aborts_replace_and_keeps_new_user_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let directory = root.join(".agents/skills/review");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("local.txt"), "original").unwrap();
    let host = WriteFailure {
        inner: LocalHost::new(),
        fail_lock_commit: false,
        fail_name: None,
        change_target_after_stage: Some(directory.join("local.txt")),
        fail_restore_lock: false,
    };
    let preview = preview_install(&host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&host, preview, &[InstallDecision::Replace], NOW);
    assert!(
        matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains("changed after confirmation"))
    );
    assert_eq!(
        fs::read_to_string(directory.join("local.txt")).unwrap(),
        "user edited while staging"
    );
    assert!(!directory.join("SKILL.md").exists());
    assert!(!root.join("skills-lock.json").exists());
    assert_eq!(
        fs::read_dir(root.join(".agents/skills")).unwrap().count(),
        1
    );
}

#[test]
fn rollback_failure_names_retained_recovery_file_and_never_claims_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let original = b"{\"version\":1,\"skills\":{},\"keep\":true}";
    fs::write(root.join("skills-lock.json"), original).unwrap();
    let host = WriteFailure {
        inner: LocalHost::new(),
        fail_lock_commit: true,
        fail_name: None,
        change_target_after_stage: None,
        fail_restore_lock: true,
    };
    let preview = preview_install(&host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&host, preview, &[InstallDecision::Install], NOW);
    assert!(
        matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains("rollback incomplete") && message.contains("source lock backup retained at"))
    );
    assert!(!root.join(".agents/skills/review").exists());
    assert!(!root.join("skills-lock.json").exists());
    let backups = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "backup")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(&backups[0]).unwrap(), original);
}

#[test]
fn relative_xdg_source_record_is_found_only_with_the_same_explicit_working_directory() {
    use lyrel_core::skills::{ScanRequest, scan};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&cwd).unwrap();
    let host = LocalHost::new();
    let target = InstallTarget::user(
        &*host,
        home.clone(),
        Some("relative-state".into()),
        cwd.clone(),
    )
    .unwrap();
    let preview = preview_install(&*host, target, vec![prepared("review")]).unwrap();
    assert_eq!(
        apply_install(&*host, preview, &[InstallDecision::Install], NOW).items[0].status,
        InstallStatus::Installed
    );
    let launches = ["omp", "pi", "kimi", "opencode", "hermes"].into_iter().map(|agent| {
        let h = home.to_string_lossy();
        (agent.to_string(), format!("HOME='{h}' USERPROFILE='{h}' PROGRAMDATA='{h}/programdata' CLAUDE_CONFIG_DIR='{h}/.claude' XDG_DATA_HOME='{h}/.data' XDG_STATE_HOME=relative-state PI_CONFIG_DIR=.omp OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{h}/.pi/agent' KIMI_CODE_HOME='{h}/.kimi-code' HERMES_HOME='{h}/.hermes' XDG_CONFIG_HOME='{h}/.config' OPENCODE_CONFIG_DIR='{h}/.config/opencode' {agent}"))
    }).collect();
    let request = ScanRequest::new(&home).with_launch_commands(launches);
    let unknown = scan(&*host, &request).unwrap();
    assert!(
        unknown
            .installations()
            .iter()
            .find(|skill| skill.name() == "review")
            .unwrap()
            .source_record()
            .is_none()
    );
    assert!(
        unknown
            .problems()
            .iter()
            .any(|message| message.contains("relative XDG_STATE_HOME"))
    );
    let known = scan(&*host, &request.with_working_directory(cwd.clone())).unwrap();
    let record = known
        .installations()
        .iter()
        .find(|skill| skill.name() == "review")
        .unwrap()
        .source_record()
        .unwrap();
    assert_eq!(record.source(), "example/skills");
    assert_eq!(
        record.lock_path(),
        cwd.join("relative-state/skills/.skill-lock.json")
    );
}

#[test]
fn rollback_does_not_delete_content_that_changed_after_the_live_rename() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let target = project(&root);
    let document = root.join(".agents/skills/review/SKILL.md");
    let host = WriteFailure {
        inner: LocalHost::new(),
        fail_lock_commit: true,
        fail_name: None,
        change_target_after_stage: Some(document.clone()),
        fail_restore_lock: false,
    };
    let preview = preview_install(&host, target, vec![prepared("review")]).unwrap();
    let report = apply_install(&host, preview, &[InstallDecision::Install], NOW);
    assert!(
        matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains("rollback incomplete") && message.contains("changed after staging; not removed"))
    );
    assert_eq!(
        fs::read_to_string(document).unwrap(),
        "user edited during lock commit"
    );
    assert!(!root.join("skills-lock.json").exists());
}

#[test]
fn project_lock_sorts_skill_keys_independent_of_install_selection_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let host = LocalHost::new();
    let preview = preview_install(
        &*host,
        project(&root),
        vec![prepared("zeta"), prepared("alpha")],
    )
    .unwrap();
    let report = apply_install(
        &*host,
        preview,
        &[InstallDecision::Install, InstallDecision::Install],
        NOW,
    );
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    assert_eq!(report.items[1].status, InstallStatus::Installed);
    let bytes = fs::read_to_string(root.join("skills-lock.json")).unwrap();
    assert!(bytes.find("\"alpha\":").unwrap() < bytes.find("\"zeta\":").unwrap());
}

#[test]
fn user_shared_install_remains_browsable_when_all_native_profiles_use_another_home() {
    use lyrel_core::skills::{ScanRequest, scan};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("selected-home");
    let native = temp.path().join("native-home");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&native).unwrap();
    let host = LocalHost::new();
    let preview = preview_install(
        &*host,
        InstallTarget::user(&*host, home.clone(), None, home.clone()).unwrap(),
        vec![prepared("review")],
    )
    .unwrap();
    assert_eq!(
        apply_install(&*host, preview, &[InstallDecision::Install], NOW).items[0].status,
        InstallStatus::Installed
    );
    let launches = ["omp", "pi", "kimi", "opencode", "hermes"].into_iter().map(|agent| {
        let h = native.to_string_lossy();
        (agent.to_string(), format!("HOME='{h}' USERPROFILE='{h}' PROGRAMDATA='{h}/programdata' CLAUDE_CONFIG_DIR='{h}/.claude' XDG_DATA_HOME='{h}/.data' XDG_STATE_HOME='{h}/.state' PI_CONFIG_DIR=.omp OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{h}/.pi/agent' KIMI_CODE_HOME='{h}/.kimi-code' HERMES_HOME='{h}/.hermes' XDG_CONFIG_HOME='{h}/.config' OPENCODE_CONFIG_DIR='{h}/.config/opencode' {agent}"))
    }).collect();
    let snapshot = scan(
        &*host,
        &ScanRequest::new(&home).with_launch_commands(launches),
    )
    .unwrap();
    let installation = snapshot
        .installations()
        .iter()
        .find(|s| s.name() == "review")
        .unwrap();
    assert_eq!(
        installation.markdown(),
        Some("---\nname: review\ndescription: Review source\n---\n# Review\n")
    );
    assert_eq!(
        installation.source_record().unwrap().source(),
        "example/skills"
    );
    assert!(
        installation
            .sources()
            .iter()
            .all(|s| s.state() == lyrel_core::skills::DiscoveryState::Unknown)
    );
}

#[test]
fn multi_skill_selection_has_one_bounded_file_budget_before_any_write() {
    let temp = tempfile::tempdir().unwrap();
    let target = project(temp.path());
    let host = LocalHost::new();
    let mut selected = vec![prepared("first"), prepared("second")];
    for skill in &mut selected {
        for file in 0..600 {
            skill.files.push(SkillFile::new(
                format!("companions/file-{file}").into(),
                Vec::new(),
            ));
        }
    }
    assert!(
        preview_install(&*host, target, selected)
            .unwrap_err()
            .to_string()
            .contains("selection")
    );
    assert!(!temp.path().join(".agents").exists());
}

#[cfg(unix)]
#[test]
#[ignore = "requires an unprivileged Unix account to exercise real shared-directory write denial"]
fn kernel_permission_denial_never_reports_installation_or_writes_a_lock() {
    use std::os::unix::fs::PermissionsExt;
    assert_ne!(
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .unwrap()
            .stdout,
        b"0\n"
    );
    let temp = tempfile::tempdir().unwrap();
    let shared_parent = temp.path().join(".agents");
    fs::create_dir(&shared_parent).unwrap();
    let host = LocalHost::new();
    let preview = preview_install(&*host, project(temp.path()), vec![prepared("review")]).unwrap();
    fs::set_permissions(&shared_parent, fs::Permissions::from_mode(0o500)).unwrap();
    let report = apply_install(&*host, preview, &[InstallDecision::Install], NOW);
    fs::set_permissions(&shared_parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(&report.items[0].status,InstallStatus::Failed(error) if error.to_lowercase().contains("permission denied"))
    );
    assert!(!shared_parent.join("skills").exists());
    assert!(!temp.path().join("skills-lock.json").exists());
}

// Host paths are deliberately foreign to the test process; only the adapter
// translates them into a real isolated filesystem.
struct PathHost {
    inner: lyrel_core::host::SharedHost,
    sandbox: tempfile::TempDir,
    id: lyrel_core::host::HostId,
    separator: char,
    disconnected: std::sync::atomic::AtomicBool,
    disconnect_after_publish: bool,
    lose_publish_reply: bool,
    lose_lock_reply: bool,
    fail_lock_after: Option<usize>,
    lock_commits: std::sync::atomic::AtomicUsize,
}
impl PathHost {
    fn new(separator: char, id: u64) -> Self {
        Self {
            inner: LocalHost::new(),
            sandbox: tempfile::tempdir().unwrap(),
            id: lyrel_core::host::HostId(id),
            separator,
            disconnected: false.into(),
            disconnect_after_publish: false,
            lose_publish_reply: false,
            lose_lock_reply: false,
            fail_lock_after: None,
            lock_commits: 0.into(),
        }
    }
    fn local(&self, path: &Path) -> std::path::PathBuf {
        assert!(
            self.is_absolute(path),
            "relative Host access: {}",
            path.display()
        );
        let text = path.to_str().unwrap();
        let text = if self.separator == '\\' {
            text.replace('\\', "/")
        } else {
            text.into()
        };
        let text = if text.as_bytes().get(1) == Some(&b':') {
            &text[2..]
        } else {
            &text
        };
        text.split('/').filter(|part| !part.is_empty()).fold(
            self.sandbox.path().to_path_buf(),
            |base, part| {
                assert!(!matches!(part, "." | ".."));
                base.join(part)
            },
        )
    }
    fn directory(&self, path: &str) {
        fs::create_dir_all(self.local(Path::new(path))).unwrap();
    }
    fn connected(&self) -> std::io::Result<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "isolated Host disconnected",
            ))
        }
    }
}
impl Host for PathHost {
    fn id(&self) -> lyrel_core::host::HostId {
        self.id
    }
    fn separator(&self) -> char {
        self.separator
    }
    fn is_absolute(&self, path: &Path) -> bool {
        let text = path.to_str().unwrap();
        if self.separator == '/' {
            text.starts_with('/')
        } else {
            text.starts_with("\\\\")
                || (text.as_bytes().get(1) == Some(&b':')
                    && matches!(text.as_bytes().get(2), Some(b'/' | b'\\')))
        }
    }
    fn is_connected(&self) -> bool {
        !self.disconnected.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn read_dir(
        &self,
        path: &Path,
        _: Option<&Path>,
    ) -> std::io::Result<Vec<lyrel_core::host::Entry>> {
        self.connected()?;
        self.inner.read_dir(&self.local(path), None)
    }
    fn stat(&self, path: &Path) -> std::io::Result<lyrel_core::host::Meta> {
        self.connected()?;
        self.inner.stat(&self.local(path))
    }
    fn read_file(&self, path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
        self.connected()?;
        self.inner.read_file(&self.local(path), max)
    }
    fn canonicalize(&self, path: &Path) -> std::io::Result<std::path::PathBuf> {
        self.stat(path)?;
        Ok(path.into())
    }
    fn search(
        &self,
        _: &[std::path::PathBuf],
        _: &str,
        _: usize,
        _: usize,
        _: bool,
    ) -> std::io::Result<Vec<lyrel_core::host::SearchHit>> {
        unreachable!()
    }
    fn search_content(
        &self,
        _: &[std::path::PathBuf],
        _: &lyrel_core::host::ContentQuery,
        _: &lyrel_core::host::ContentLimits,
    ) -> std::io::Result<lyrel_core::host::ContentResults> {
        unreachable!()
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<lyrel_core::host::Meta> {
        self.connected()?;
        self.inner.write_file(&self.local(path), bytes)
    }
    fn create_file_new(&self, path: &Path) -> std::io::Result<()> {
        self.connected()?;
        self.inner.create_file_new(&self.local(path))
    }
    fn create_dir(&self, path: &Path, recursive: bool) -> std::io::Result<()> {
        self.connected()?;
        self.inner.create_dir(&self.local(path), recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.connected()?;
        let lock_commit = from.to_str().unwrap().ends_with(".stage")
            && to.to_str().unwrap().ends_with("skills-lock.json");
        if lock_commit
            && self.fail_lock_after.is_some_and(|limit| {
                self.lock_commits.load(std::sync::atomic::Ordering::SeqCst) >= limit
            })
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "isolated lock commit denied",
            ));
        }
        self.inner.rename(&self.local(from), &self.local(to))?;
        if lock_commit {
            self.lock_commits
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if (self.lose_publish_reply && to.to_str().unwrap().ends_with("review"))
            || (self.lose_lock_reply && to.to_str().unwrap().ends_with("skills-lock.json"))
        {
            self.disconnected
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "isolated rename reply lost",
            ));
        }
        if self.disconnect_after_publish && to.to_str().unwrap().ends_with("review") {
            self.disconnected
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
    fn remove(&self, path: &Path, recursive: bool) -> std::io::Result<()> {
        self.connected()?;
        self.inner.remove(&self.local(path), recursive)
    }
    fn repo_root(&self, _: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
        unreachable!()
    }
    fn git(&self, _: &Path, _: &[&str]) -> std::io::Result<lyrel_core::host::Output> {
        unreachable!()
    }
    fn shells(&self) -> std::io::Result<lyrel_core::core::shells::ShellInventory> {
        unreachable!()
    }
    fn watch(&self, _: &[std::path::PathBuf]) -> std::io::Result<lyrel_core::host::WatchSub> {
        unreachable!()
    }
}

#[test]
fn host_path_install_resolves_roots_state_ancestors_and_nested_staging_on_the_target() {
    for (separator, home, cwd, state, expected_lock, destination) in [
        (
            '/',
            "/home/shared",
            "/workspace/repo",
            "../state",
            "/workspace/state/skills/.skill-lock.json",
            "/home/shared/.agents/skills/review",
        ),
        (
            '\\',
            r"C:\Users\shared",
            r"C:\workspace\repo",
            r"..\state",
            r"C:\workspace\state\skills\.skill-lock.json",
            r"C:\Users\shared\.agents\skills\review",
        ),
        (
            '\\',
            r"\\server\share\home",
            r"\\server\share\workspace\repo",
            r"..\state",
            r"\\server\share\workspace\state\skills\.skill-lock.json",
            r"\\server\share\home\.agents\skills\review",
        ),
    ] {
        let host = PathHost::new(separator, 41);
        host.directory(home);
        host.directory(cwd);
        let target =
            InstallTarget::user(&host, home.into(), Some(state.into()), cwd.into()).unwrap();
        assert_eq!(target.lock_path().to_str().unwrap(), expected_lock);
        let mut skill = prepared("review");
        skill.files.push(SkillFile::new(
            "references/nested/guide.txt".into(),
            b"target Host".to_vec(),
        ));
        let preview = preview_install(&host, target, vec![skill]).unwrap();
        assert_eq!(
            preview.items()[0].destination().to_str().unwrap(),
            destination
        );
        let report = apply_install(&host, preview, &[InstallDecision::Install], NOW);
        assert_eq!(report.items[0].status, InstallStatus::Installed);
        assert_eq!(
            fs::read(host.local(&host.join(Path::new(destination), "references/nested/guide.txt")))
                .unwrap(),
            b"target Host"
        );
        let lock: serde_json::Value =
            serde_json::from_slice(&fs::read(host.local(Path::new(expected_lock))).unwrap())
                .unwrap();
        assert_eq!(lock["skills"]["review"]["source"], "example/skills");
    }
}

#[test]
fn same_named_host_paths_cannot_retarget_a_constructor_or_confirmation_and_cancel_writes_nothing() {
    let first = PathHost::new('/', 51);
    let second = PathHost::new('/', 52);
    for host in [&first, &second] {
        host.directory("/project");
    }
    let target = InstallTarget::project(&first, "/project".into()).unwrap();
    assert!(preview_install(&second, target.clone(), vec![prepared("review")]).is_err());
    let preview = preview_install(&first, target.clone(), vec![prepared("review")]).unwrap();
    let kept = apply_install(&first, preview, &[], NOW);
    assert_eq!(kept.items[0].status, InstallStatus::Kept);
    let preview = preview_install(&first, target.clone(), vec![prepared("review")]).unwrap();
    let wrong = apply_install(&second, preview, &[InstallDecision::Install], NOW);
    assert!(matches!(wrong.items[0].status, InstallStatus::Failed(_)));
    for host in [&first, &second] {
        assert_eq!(
            fs::read_dir(host.local(Path::new("/project")))
                .unwrap()
                .count(),
            0
        );
    }
    let preview = preview_install(&first, target, vec![prepared("review")]).unwrap();
    assert_eq!(
        apply_install(&first, preview, &[InstallDecision::Install], NOW).items[0].status,
        InstallStatus::Installed
    );
    assert_eq!(
        fs::read_dir(second.local(Path::new("/project")))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn remote_relative_state_requires_explicit_host_cwd_and_never_uses_desktop_cwd() {
    let host = PathHost::new('/', 61);
    assert!(
        InstallTarget::user(
            &host,
            "/home/shared".into(),
            Some("state".into()),
            "".into()
        )
        .is_err()
    );
    assert!(
        InstallTarget::user(
            &host,
            "/home/shared".into(),
            Some("state".into()),
            "relative".into()
        )
        .is_err()
    );
    assert!(InstallTarget::project(&host, "/home/../private".into()).is_err());
    let windows = PathHost::new('\\', 62);
    assert!(InstallTarget::project(&windows, r"C:\home\..\private".into()).is_err());
    assert!(InstallTarget::project(&windows, r"C:relative".into()).is_err());
    assert!(
        InstallTarget::user(
            &windows,
            r"C:\home".into(),
            Some(r"..\..\state".into()),
            r"C:\workspace".into()
        )
        .is_err()
    );
}

#[test]
fn disconnect_after_live_publish_reports_exact_unrecorded_target_and_recovery_without_retrying_next_item()
 {
    let mut host = PathHost::new('\\', 71);
    host.disconnect_after_publish = true;
    host.directory(r"C:\project");
    let target = InstallTarget::project(&host, r"C:\project".into()).unwrap();
    let preview =
        preview_install(&host, target, vec![prepared("review"), prepared("second")]).unwrap();
    let report = apply_install(
        &host,
        preview,
        &[InstallDecision::Install, InstallDecision::Install],
        NOW,
    );
    assert!(
        matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains(r"C:\project\.agents\skills\review") && message.contains("rollback incomplete") && message.contains("Reconnect") && message.contains("review") )
    );
    assert!(matches!(&report.items[1].status, InstallStatus::Failed(_)));
    assert!(
        host.local(Path::new(r"C:\project\.agents\skills\review\SKILL.md"))
            .exists()
    );
    assert!(
        !host
            .local(Path::new(r"C:\project\skills-lock.json"))
            .exists()
    );
    assert!(
        !host
            .local(Path::new(r"C:\project\.agents\skills\second"))
            .exists()
    );
}

#[test]
fn lost_live_rename_reply_preserves_uncertain_content_and_reports_recovery_paths() {
    for lose_lock_reply in [false, true] {
        let mut host = PathHost::new('/', 81);
        host.lose_publish_reply = !lose_lock_reply;
        host.lose_lock_reply = lose_lock_reply;
        host.directory("/project");
        let target = InstallTarget::project(&host, "/project".into()).unwrap();
        let preview = preview_install(&host, target, vec![prepared("review")]).unwrap();
        let report = apply_install(&host, preview, &[InstallDecision::Install], NOW);
        assert!(
            matches!(&report.items[0].status, InstallStatus::Failed(message) if message.contains("outcome unknown") && message.contains("/project/.agents/skills/review") && message.contains("/project/skills-lock.json") && message.contains("Reconnect"))
        );
        assert!(
            host.local(Path::new("/project/.agents/skills/review/SKILL.md"))
                .exists()
        );
        assert_eq!(
            host.local(Path::new("/project/skills-lock.json")).exists(),
            lose_lock_reply
        );
    }
}

#[cfg(any(unix, windows))]
#[test]
fn foreign_host_ancestors_and_dangling_entries_cannot_redirect_shared_writes() {
    for (separator, project_path, agents_path) in [
        ('/', "/project", "/project/.agents"),
        ('\\', r"C:\project", r"C:\project\.agents"),
    ] {
        let host = PathHost::new(separator, 91);
        host.directory(project_path);
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep.txt"), "unrelated").unwrap();
        let target = InstallTarget::project(&host, project_path.into()).unwrap();
        let preview = preview_install(&host, target.clone(), vec![prepared("review")]).unwrap();
        directory_link(outside.path(), &host.local(Path::new(agents_path)));
        let report = apply_install(&host, preview, &[InstallDecision::Install], NOW);
        assert!(matches!(report.items[0].status, InstallStatus::Failed(_)));
        assert!(preview_install(&host, target, vec![prepared("review")]).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
            "unrelated"
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
        assert!(
            !host
                .local(&host.join(Path::new(project_path), "skills-lock.json"))
                .exists()
        );
    }
}

#[test]
fn fully_qualified_host_roots_allow_safe_project_installs_without_client_root_parsing() {
    for (separator, root) in [('/', "/"), ('\\', r"C:\"), ('\\', r"\\server\share\")] {
        let host = PathHost::new(separator, 101);
        host.directory(root);
        let target = InstallTarget::project(&host, root.into()).unwrap();
        let preview = preview_install(&host, target, vec![prepared("review")]).unwrap();
        let report = apply_install(&host, preview, &[InstallDecision::Install], NOW);
        assert_eq!(report.items[0].status, InstallStatus::Installed);
        assert!(
            host.local(&host.join(Path::new(root), ".agents/skills/review/SKILL.md"))
                .exists()
        );
        assert!(
            host.local(&host.join(Path::new(root), "skills-lock.json"))
                .exists()
        );
    }
}

#[test]
fn second_remote_lock_failure_preserves_first_committed_item_and_rolls_back_only_second() {
    let mut host = PathHost::new('\\', 111);
    host.fail_lock_after = Some(1);
    host.directory(r"C:\project");
    let target = InstallTarget::project(&host, r"C:\project".into()).unwrap();
    let preview =
        preview_install(&host, target, vec![prepared("first"), prepared("second")]).unwrap();
    let report = apply_install(
        &host,
        preview,
        &[InstallDecision::Install, InstallDecision::Install],
        NOW,
    );
    assert_eq!(report.items[0].status, InstallStatus::Installed);
    assert_eq!(
        report.items[1].destination.to_str().unwrap(),
        r"C:\project\.agents\skills\second"
    );
    assert!(
        matches!(&report.items[1].status, InstallStatus::Failed(message) if message.contains("isolated lock commit denied"))
    );
    assert!(
        host.local(Path::new(r"C:\project\.agents\skills\first\SKILL.md"))
            .exists()
    );
    assert!(
        !host
            .local(Path::new(r"C:\project\.agents\skills\second"))
            .exists()
    );
    let lock: serde_json::Value = serde_json::from_slice(
        &fs::read(host.local(Path::new(r"C:\project\skills-lock.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(lock["skills"]["first"]["source"], "example/skills");
    assert!(lock["skills"].get("second").is_none());
}
