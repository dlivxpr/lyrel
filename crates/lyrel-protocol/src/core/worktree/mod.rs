//! Worktrees lyrel makes: one per task, under `<repo>/.lyrel/worktrees/<name>`.
//!
//! - **Create** starts from the remote's default branch, fetched first
//!   ([`defaults`]): a new task starts from what is merged, not from whatever
//!   the current checkout happens to be on. Then the `.worktreeinclude` files
//!   come over ([`include`]), and the first pane runs `.lyrel/setup` before the
//!   agent ([`setup`]).
//! - **Remove** files a snapshot of the checkout, uncommitted work included,
//!   under `refs/lyrel/trash/<name>` before deleting anything, so even "Discard
//!   Changes & Remove" can be undone with `git checkout refs/lyrel/trash/<name>`.
//! - **Names** are never reused: that snapshot ref retires the name. An agent
//!   keys its session history by directory, and a new worktree at an old path
//!   would open onto another task's conversations.

pub mod include;
pub mod setup;

use std::path::{Path, PathBuf};

use crate::core::codename::Names;
use crate::core::git::git_path;
use crate::host::Host;

/// Where removed worktrees leave their last state.
const TRASH_REFS: &str = "refs/lyrel/trash";

#[derive(Debug)]
pub struct NewWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub main_root: PathBuf,
    pub carried: include::Carried,
}

/// What [`remove`] did beyond deleting the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// The ref holding the checkout's last state.
    pub snapshot: Option<String>,
    /// `git branch -d` refused: the branch has commits nothing else has.
    pub branch_kept: bool,
}

#[derive(Debug, Clone)]
pub struct WorktreeRequest {
    pub name: String,
    pub branch: String,
    pub base: String,
}

#[derive(Debug, Clone)]
pub struct WorktreeDefaults {
    pub name: String,
    pub base: String,
    pub dir: PathBuf,
    /// The repo has a `.lyrel/setup`.
    pub has_setup: bool,
    /// Without one, the install its lockfile suggests.
    pub setup_hint: Option<&'static str>,
}

fn managed_root(host: &dyn Host, main_root: &Path) -> PathBuf {
    host.join(&host.join(main_root, ".lyrel"), "worktrees")
}

fn git(host: &dyn Host, dir: &Path, args: &[&str]) -> Result<String, String> {
    match host.git(dir, args) {
        Ok(out) if out.success() => Ok(out.stdout_trimmed()),
        Ok(out) => Err(out.stderr_trimmed()),
        Err(e) => Err(format!("failed to run git: {e}")),
    }
}

fn branch_exists(host: &dyn Host, repo_root: &Path, name: &str) -> bool {
    git(
        host,
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

#[derive(Debug, Clone)]
pub struct ManagedWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub main_root: PathBuf,
    pub dirty: bool,
}

pub fn managed(host: &dyn Host, cwd: &Path) -> Option<ManagedWorktree> {
    let cwd = host.canonicalize(cwd).ok()?;
    let suffix = host.join(Path::new(".lyrel"), "worktrees");
    if !cwd.ancestors().any(|a| a.ends_with(&suffix)) {
        return None;
    }
    let path = git_path(
        host,
        &git(host, &cwd, &["rev-parse", "--show-toplevel"]).ok()?,
    );
    let main_root = git(
        host,
        &path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(|d| git_path(host, &d))?
    .parent()?
    .to_path_buf();
    if !path.starts_with(managed_root(host, &main_root)) {
        return None;
    }
    let branch = git(host, &path, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let dirty = !git(host, &path, &["status", "--porcelain"])
        .ok()?
        .is_empty();
    Some(ManagedWorktree {
        path,
        branch,
        main_root,
        dirty,
    })
}

pub fn occupied(host: &dyn Host, path: &Path, cwds: &[PathBuf]) -> bool {
    let Ok(path) = host.canonicalize(path) else {
        return false;
    };
    cwds.iter()
        .any(|c| host.canonicalize(c).is_ok_and(|c| c.starts_with(&path)))
}

/// Whether a pane sits inside `path`, asked on the Host that owns both.
/// Unlike [`occupied`], which answers `false` when a path cannot be
/// canonicalized, a failed read here is `Err`: a check that did not run is
/// not proof of absence, and a removal authorization must treat it as a
/// block.
pub fn occupancy(host: &dyn Host, path: &Path, cwds: &[PathBuf]) -> Result<bool, String> {
    let root = host
        .canonicalize(path)
        .map_err(|e| format!("could not verify {}: {e}", path.display()))?;
    for cwd in cwds {
        let cwd = host
            .canonicalize(cwd)
            .map_err(|e| format!("could not verify {}: {e}", cwd.display()))?;
        if cwd.starts_with(&root) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What became of a removed checkout's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchOutcome {
    /// The checkout had a detached HEAD: there is no branch to clean up.
    Detached,
    /// `git branch -d` deleted the branch — it was fully merged.
    Deleted,
    /// `git branch -d` refused: unmerged commits remain, so the branch stays.
    Kept,
}

/// A clean checkout the picker removed from disk. The recovery reference and
/// the branch outcome are distinct facts; a result tells them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRemoval {
    /// The create-only reference holding the checkout's last commit.
    pub recovery: String,
    pub branch: BranchOutcome,
}

/// The object name `update-ref` expects for a create-only write.
const NO_OID: &str = "0000000000000000000000000000000000000000";

/// Remove one linked checkout — the picker's clean, never-forced removal
/// (#352). Unlike [`remove`], which only touches worktrees lyrel made, this
/// takes any linked checkout, including one created outside lyrel.
///
/// The main checkout is refused and a dirty checkout is refused (dirty
/// removal is #353); nothing here runs forced. Before anything is deleted
/// the checkout's last commit is filed under `refs/lyrel/trash/<name>` with
/// a create-only `update-ref`: an existing recovery reference — maybe from a
/// different checkout that shared the basename — is never overwritten, a
/// numeric suffix is tried instead, and when no recovery reference can be
/// created nothing is removed. Only after the checkout is gone does the
/// branch — never a basename, which is not checkout identity — get its safe
/// cleanup, `git branch -d` only. A detached HEAD names no branch and gets
/// no branch work.
pub fn remove_checkout(host: &dyn Host, checkout: &Listed) -> Result<CheckoutRemoval, String> {
    if checkout.main {
        return Err("the main checkout cannot be removed".to_string());
    }
    let canonical = host
        .canonicalize(&checkout.path)
        .map_err(|e| format!("could not verify {}: {e}", checkout.path.display()))?;
    let listed = list(host, &canonical)
        .map_err(|e| format!("could not verify the checkout's registration: {e}"))?;
    let found = listed
        .iter()
        .find(|wt| host.canonicalize(&wt.path).ok().as_deref() == Some(canonical.as_path()))
        .ok_or_else(|| format!("{} is no longer a registered checkout", canonical.display()))?;
    if found.main {
        return Err("the main checkout cannot be removed".to_string());
    }
    let dirty = git(host, &found.path, &["status", "--porcelain"])
        .map_err(|e| format!("could not verify the checkout is clean: {e}"))?;
    if !dirty.is_empty() {
        return Err(format!(
            "{} has uncommitted changes and will not be removed",
            canonical.display()
        ));
    }
    let head = git(host, &found.path, &["rev-parse", "HEAD"])
        .map_err(|e| format!("could not read the checkout's last commit: {e}"))?;
    let name = canonical
        .file_name()
        .ok_or("the checkout has no name")?
        .to_string_lossy()
        .into_owned();
    let main_root = git(
        host,
        &found.path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(|d| git_path(host, &d))
    .and_then(|d| d.parent().map(Path::to_path_buf))
    .ok_or_else(|| format!("could not find the repository of {}", canonical.display()))?;
    let recovery = create_recovery_ref(host, &main_root, &name, &head)?;
    let target = found
        .path
        .to_str()
        .ok_or("checkout path is not valid UTF-8")?;
    if let Err(e) = git(host, &main_root, &["worktree", "remove", target]) {
        // The checkout stays, so the recovery reference must not retire the
        // name: drop the ref we just created, best effort, expecting the
        // value we wrote so a racing writer is never undone.
        let _ = git(host, &main_root, &["update-ref", "-d", &recovery, &head]);
        return Err(e);
    }
    let branch = match found.branch.as_deref() {
        Some(branch) if branch != "HEAD" => {
            if git(host, &main_root, &["branch", "-d", branch]).is_ok() {
                BranchOutcome::Deleted
            } else {
                BranchOutcome::Kept
            }
        }
        _ => BranchOutcome::Detached,
    };
    Ok(CheckoutRemoval { recovery, branch })
}

/// File `oid` under `refs/lyrel/trash/<name>`, creating only. When the name
/// is taken — an older removal, or another checkout with the same basename
/// that got there first — numeric suffixes are tried; a reference that
/// already exists is never moved.
fn create_recovery_ref(
    host: &dyn Host,
    repo: &Path,
    name: &str,
    oid: &str,
) -> Result<String, String> {
    for n in 1..100 {
        let refname = if n == 1 {
            format!("{TRASH_REFS}/{name}")
        } else {
            format!("{TRASH_REFS}/{name}-{n}")
        };
        if git(host, repo, &["update-ref", &refname, oid, NO_OID]).is_ok() {
            return Ok(refname);
        }
    }
    Err(format!(
        "could not create a recovery reference under {TRASH_REFS}: every name is taken"
    ))
}

pub fn remove(host: &dyn Host, wt: &ManagedWorktree, force: bool) -> Result<Removed, String> {
    let path = wt.path.to_str().ok_or("worktree path is not valid UTF-8")?;
    if wt.dirty && !force {
        return Err(format!("{} has uncommitted changes", wt.path.display()));
    }
    // Uncommitted work is only thrown away once it is safe somewhere else.
    let snapshot = match snapshot(host, wt) {
        Ok(r) => Some(r),
        Err(e) if wt.dirty => {
            return Err(format!(
                "could not save the uncommitted changes, so nothing was removed: {e}"
            ));
        }
        Err(e) => {
            log::warn!("no snapshot of {}: {e}", wt.path.display());
            None
        }
    };
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);
    git(host, &wt.main_root, &args)?;
    let branch_kept =
        wt.branch != "HEAD" && git(host, &wt.main_root, &["branch", "-d", &wt.branch]).is_err();
    Ok(Removed {
        snapshot,
        branch_kept,
    })
}

/// File the checkout's state under [`TRASH_REFS`]`/<name>`: its commit, or a
/// stash-shaped commit on top of it when there are changes, untracked files
/// included.
fn snapshot(host: &dyn Host, wt: &ManagedWorktree) -> Result<String, String> {
    let name = wt
        .path
        .file_name()
        .ok_or("worktree has no name")?
        .to_string_lossy()
        .into_owned();
    let head = || git(host, &wt.path, &["rev-parse", "HEAD"]);
    let commit = if wt.dirty {
        // `stash create` only sees the index. The checkout is about to go, so
        // staging everything in it costs nothing.
        git(host, &wt.path, &["add", "-A"])?;
        let made = git(
            host,
            &wt.path,
            &["stash", "create", "lyrel: removed worktree"],
        )?;
        if made.is_empty() { head()? } else { made }
    } else {
        head()?
    };
    let refname = format!("{TRASH_REFS}/{name}");
    git(host, &wt.main_root, &["update-ref", &refname, &commit])?;
    Ok(refname)
}

/// One checkout of a repo, as `git worktree list` knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    /// The repo's main checkout.
    pub main: bool,
    /// Made by lyrel, under `.lyrel/worktrees/`.
    pub managed: bool,
}

/// Every checkout of the repo `cwd` is in, the main one first.
pub fn list(host: &dyn Host, cwd: &Path) -> Result<Vec<Listed>, String> {
    let (_, dir) = repo_dir(host, cwd)?;
    let out = git(host, cwd, &["worktree", "list", "--porcelain"])?;
    let mut found: Vec<Listed> = Vec::new();
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let path = git_path(host, path);
            found.push(Listed {
                managed: path.starts_with(&dir),
                main: found.is_empty(),
                path,
                branch: None,
            });
        } else if let (Some(branch), Some(last)) = (line.strip_prefix("branch "), found.last_mut())
        {
            last.branch = Some(branch.trim_start_matches("refs/heads/").to_string());
        }
    }
    Ok(found)
}

/// The lyrel worktree `target` names: a path, or a name under the repo's
/// `.lyrel/worktrees/`.
pub fn find(host: &dyn Host, cwd: &Path, target: &str) -> Result<ManagedWorktree, String> {
    let as_path = Path::new(target);
    let path = if host.is_absolute(as_path) || target.contains(['/', '\\']) {
        as_path.to_path_buf()
    } else {
        host.join(&repo_dir(host, cwd)?.1, target)
    };
    if !host.exists(&path) {
        return Err(format!("no worktree at {}", path.display()));
    }
    managed(host, &path).ok_or_else(|| format!("{} is not a worktree lyrel made", path.display()))
}

/// Names removed worktrees left behind.
fn retired(host: &dyn Host, repo_root: &Path) -> Vec<String> {
    git(
        host,
        repo_root,
        &["for-each-ref", "--format=%(refname:strip=3)", TRASH_REFS],
    )
    .map(|out| out.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

fn repo_dir(host: &dyn Host, cwd: &Path) -> Result<(PathBuf, PathBuf), String> {
    let repo_root = git(host, cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "not inside a git repository".to_string())?;
    let repo_root = git_path(host, &repo_root);
    let main_root = git(
        host,
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(|d| git_path(host, &d))
    .and_then(|d| d.parent().map(Path::to_path_buf))
    .unwrap_or_else(|| repo_root.clone());
    let dir = managed_root(host, &main_root);
    Ok((repo_root, dir))
}

pub fn defaults(host: &dyn Host, cwd: &Path) -> Result<WorktreeDefaults, String> {
    let (repo_root, dir) = repo_dir(host, cwd)?;
    let retired = retired(host, &repo_root);
    let name = Names::new().unique(|name| {
        retired.iter().any(|r| r == name)
            || branch_exists(host, &repo_root, name)
            || host.exists(&host.join(&dir, name))
    });
    let has_setup = setup::find(host, &repo_root).is_some();
    Ok(WorktreeDefaults {
        name,
        base: default_base(host, &repo_root),
        dir,
        has_setup,
        setup_hint: (!has_setup)
            .then(|| setup::hint(host, &repo_root))
            .flatten(),
    })
}

/// The remote's default branch (`origin/main`), or the current branch when
/// there is no remote to start from.
fn default_base(host: &dyn Host, repo_root: &Path) -> String {
    let verify = |r: &str| git(host, repo_root, &["rev-parse", "--verify", "--quiet", r]).is_ok();
    git(
        host,
        repo_root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .or_else(|| {
        ["main", "master"]
            .into_iter()
            .find(|b| verify(&format!("refs/remotes/origin/{b}")))
            .map(|b| format!("origin/{b}"))
    })
    .unwrap_or_else(|| {
        git(host, repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_else(|_| "HEAD".to_string())
    })
}

/// Bring `base` up to date when it is a remote-tracking branch. Best effort:
/// offline, the local copy of the ref is still a fine place to start.
fn refresh(host: &dyn Host, repo_root: &Path, base: &str) {
    let tracking = format!("refs/remotes/{base}");
    if git(
        host,
        repo_root,
        &["rev-parse", "--verify", "--quiet", &tracking],
    )
    .is_err()
    {
        return;
    }
    let Some((remote, branch)) = base.split_once('/') else {
        return;
    };
    let _ = host.git_with_deadline(
        repo_root,
        &["fetch", "--quiet", "--no-tags", remote, branch],
        std::time::Duration::from_secs(15),
    );
}

/// Keep `.lyrel/worktrees/` out of `git status` without a file in the tree,
/// so the repo's `.lyrel/setup` stays committable. Repos set up before this
/// carry a `.lyrel/.gitignore` of `*`, which hid the setup script too.
fn exclude(host: &dyn Host, main_root: &Path) {
    let legacy = host.join(&host.join(main_root, ".lyrel"), ".gitignore");
    if host.read_file(&legacy, 64).is_ok_and(|b| b == b"*\n") {
        let _ = host.remove(&legacy, false);
    }
    let Ok(common) = git(
        host,
        main_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) else {
        return;
    };
    let info = host.join(&git_path(host, &common), "info");
    let file = host.join(&info, "exclude");
    const LINE: &str = "/.lyrel/worktrees/";
    let current = host.read_file(&file, 1 << 20).unwrap_or_default();
    let current = String::from_utf8_lossy(&current);
    if current.lines().any(|l| l.trim() == LINE) {
        return;
    }
    let mut next = current.into_owned();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(LINE);
    next.push('\n');
    let _ = host.create_dir(&info, true);
    let _ = host.write_file(&file, next.as_bytes());
}

pub fn create(host: &dyn Host, cwd: &Path, req: &WorktreeRequest) -> Result<NewWorktree, String> {
    if req.name.is_empty() || req.name == "." || req.name == ".." || req.name.contains(['/', '\\'])
    {
        return Err(format!("invalid worktree name \"{}\"", req.name));
    }
    let (repo_root, dir) = repo_dir(host, cwd)?;
    let main_root = dir
        .parent()
        .and_then(Path::parent)
        .expect(".lyrel/worktrees sits two levels under the main checkout")
        .to_path_buf();
    host.create_dir(&dir, true)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    exclude(host, &main_root);

    let path = host.join(&dir, &req.name);
    if host.exists(&path) {
        return Err(format!("{} already exists", path.display()));
    }
    refresh(host, &repo_root, &req.base);
    git(
        host,
        &repo_root,
        &[
            "worktree",
            "add",
            // Starting from `origin/main` must not make the branch track it:
            // its first push belongs on a branch of its own name.
            "--no-track",
            "-b",
            &req.branch,
            path.to_str().ok_or("worktree path is not valid UTF-8")?,
            &req.base,
        ],
    )?;
    let carried = include::carry(host, &main_root, &path);
    Ok(NewWorktree {
        path,
        branch: req.branch.clone(),
        main_root,
        carried,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lyrel-wt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plain(p: &Path) -> PathBuf {
        let s = p.to_string_lossy();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
    }

    fn sh(dir: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new(args[0])
                .args(&args[1..])
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success(),
            "command failed: {args:?}"
        );
    }

    fn temp_repo(name: &str) -> PathBuf {
        let dir = scratch(name);
        sh(&dir, &["git", "init", "-q"]);
        assert!(crate::core::git::test_support::pin_repo_config(&dir));
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        sh(&dir, &["git", "add", "."]);
        sh(&dir, &["git", "commit", "-q", "-m", "init"]);
        dir
    }

    fn h() -> crate::host::SharedHost {
        crate::host::local::LocalHost::new()
    }

    fn req(name: &str) -> WorktreeRequest {
        WorktreeRequest {
            name: name.into(),
            branch: name.into(),
            base: "HEAD".into(),
        }
    }

    #[test]
    fn defaults_proposes_fresh_name_current_branch_and_target_dir() {
        let h = h();
        let repo = temp_repo("dflt");
        let d = defaults(&*h, &repo).unwrap();
        assert!(!branch_exists(&*h, &repo, &d.name));
        let head = git(&*h, &repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(d.base, head);
        let canon = plain(&std::fs::canonicalize(&repo).unwrap());
        assert_eq!(plain(&d.dir), canon.join(".lyrel").join("worktrees"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_makes_worktree_on_new_branch_inside_the_repo() {
        let h = h();
        let repo = temp_repo("repo");
        let wt = create(&*h, &repo, &req("quiet-otter")).unwrap();
        assert!(wt.path.join("a.txt").exists());
        assert!(branch_exists(&*h, &repo, &wt.branch));
        let canon = plain(&std::fs::canonicalize(&repo).unwrap());
        assert_eq!(plain(&wt.path), canon.join(".lyrel/worktrees/quiet-otter"));
        let head = git(&*h, &wt.path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(head, wt.branch);
        assert!(!canon.join(".lyrel/.gitignore").exists());
        let exclude = std::fs::read_to_string(canon.join(".git/info/exclude")).unwrap();
        assert_eq!(
            exclude
                .lines()
                .filter(|l| *l == "/.lyrel/worktrees/")
                .count(),
            1
        );
        assert_eq!(git(&*h, &repo, &["status", "--porcelain"]).unwrap(), "");
        assert!(
            create(&*h, &repo, &req("quiet-otter"))
                .unwrap_err()
                .contains("already exists")
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_honors_custom_branch_and_base() {
        let h = h();
        let repo = temp_repo("base");
        sh(&repo, &["git", "branch", "stable"]);
        std::fs::write(repo.join("b.txt"), "b").unwrap();
        sh(&repo, &["git", "add", "."]);
        sh(&repo, &["git", "commit", "-q", "-m", "second"]);
        let wt = create(
            &*h,
            &repo,
            &WorktreeRequest {
                name: "my-dir".into(),
                branch: "feat/my-branch".into(),
                base: "stable".into(),
            },
        )
        .unwrap();
        assert_eq!(wt.path.file_name().unwrap().to_str().unwrap(), "my-dir");
        let head = git(&*h, &wt.path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(head, "feat/my-branch");
        assert!(wt.path.join("a.txt").exists());
        assert!(!wt.path.join("b.txt").exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_rejects_escaping_names() {
        let h = h();
        let repo = temp_repo("names");
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            let mut r = req("x");
            r.name = bad.into();
            assert!(
                create(&*h, &repo, &r)
                    .unwrap_err()
                    .contains("invalid worktree name"),
                "{bad:?} should be rejected"
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_from_a_linked_worktree_lands_in_the_main_repo() {
        let h = h();
        let repo = temp_repo("nest");
        let first = create(&*h, &repo, &req("first-wt")).unwrap();
        let second = create(&*h, &first.path, &req("second-wt")).unwrap();
        assert_eq!(second.path.parent().unwrap(), first.path.parent().unwrap());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn managed_resolves_managed_checkouts_and_remove_cleans_up() {
        let h = h();
        let repo = temp_repo("mg");
        let wt = create(&*h, &repo, &req("mg-wt")).unwrap();
        assert!(managed(&*h, &repo).is_none());
        let own = scratch("mg-own");
        let _ = std::fs::remove_dir_all(&own);
        sh(
            &repo,
            &[
                "git",
                "worktree",
                "add",
                "-b",
                "own-branch",
                own.to_str().unwrap(),
            ],
        );
        assert!(managed(&*h, &own).is_none());
        let sub = wt.path.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let m = managed(&*h, &sub).unwrap();
        assert_eq!(m.branch, wt.branch);
        assert_eq!(m.path, wt.path);
        assert!(!m.dirty);
        std::fs::write(wt.path.join("b.txt"), "b").unwrap();
        let m = managed(&*h, &wt.path).unwrap();
        assert!(m.dirty);
        assert!(remove(&*h, &m, false).is_err());
        remove(&*h, &m, true).unwrap();
        assert!(!wt.path.exists());
        assert!(!branch_exists(&*h, &repo, &wt.branch));
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&own);
    }

    #[test]
    fn writes_survive_the_optional_locks_invariant() {
        let h = h();
        let repo = temp_repo("locks");

        let wt = create(&*h, &repo, &req("lock-wt")).unwrap();
        assert!(wt.path.join("a.txt").exists());

        let list = git(&*h, &repo, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(
            list.lines()
                .any(|l| l.starts_with("branch ") && l.ends_with(&wt.branch)),
            "worktree list must show the new checkout: {list}"
        );

        std::fs::write(wt.path.join("c.txt"), "c").unwrap();
        git(&*h, &wt.path, &["add", "."]).unwrap();
        git(
            &*h,
            &wt.path,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "c",
            ],
        )
        .unwrap();
        assert_eq!(git(&*h, &wt.path, &["status", "--porcelain"]).unwrap(), "");

        let m = managed(&*h, &wt.path).unwrap();
        remove(&*h, &m, false).unwrap();
        assert!(!wt.path.exists());
        assert!(branch_exists(&*h, &repo, &wt.branch));
        let plain_wt = create(&*h, &repo, &req("lock-wt2")).unwrap();
        let m = managed(&*h, &plain_wt.path).unwrap();
        remove(&*h, &m, false).unwrap();
        assert!(!branch_exists(&*h, &repo, &plain_wt.branch));

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn occupied_detects_live_cwds_inside_the_worktree() {
        let h = h();
        let repo = temp_repo("occ");
        let wt = create(&*h, &repo, &req("occ-wt")).unwrap();
        let inside = wt.path.join("deep");
        std::fs::create_dir_all(&inside).unwrap();
        assert!(occupied(&*h, &wt.path, &[repo.clone(), inside]));
        assert!(!occupied(&*h, &wt.path, std::slice::from_ref(&repo)));
        assert!(!occupied(&*h, &wt.path, &[wt.path.join("gone")]));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_outside_a_repo_errors() {
        let h = h();
        let plain = scratch("plain");
        let err = create(&*h, &plain, &req("x")).unwrap_err();
        assert_eq!(err, "not inside a git repository");
        assert_eq!(
            defaults(&*h, &plain).unwrap_err(),
            "not inside a git repository"
        );
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[test]
    fn create_carries_worktreeinclude_matches_that_git_ignores() {
        let h = h();
        let repo = temp_repo("incl");
        std::fs::write(repo.join(".gitignore"), ".env*\nnode_modules/\nlocal/\n").unwrap();
        std::fs::write(
            repo.join(".worktreeinclude"),
            ".env\n.env.local\nnode_modules/\n/local/cfg/*.json\nloose.txt\n",
        )
        .unwrap();
        sh(&repo, &["git", "add", "."]);
        sh(&repo, &["git", "commit", "-q", "-m", "include"]);
        for (file, body) in [
            (".env", "A=1"),
            (".env.local", "B=2"),
            (".env.other", "C=3"),
            ("node_modules/x/y.js", "y"),
            ("local/cfg/a.json", "{}"),
            ("local/cfg/b.txt", "b"),
            ("loose.txt", "not ignored"),
        ] {
            let p = repo.join(file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink("x/y.js", repo.join("node_modules/link.js")).unwrap();

        let wt = create(&*h, &repo, &req("incl-wt")).unwrap();
        let mut copied = wt.carried.copied.clone();
        copied.sort();
        assert_eq!(
            copied,
            [".env", ".env.local", "local/cfg/a.json", "node_modules"]
        );
        assert!(wt.carried.skipped.is_empty(), "{:?}", wt.carried.skipped);
        assert_eq!(
            std::fs::read_to_string(wt.path.join(".env")).unwrap(),
            "A=1"
        );
        assert!(wt.path.join("node_modules/x/y.js").exists());
        #[cfg(unix)]
        assert!(
            std::fs::symlink_metadata(wt.path.join("node_modules/link.js"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(wt.path.join("local/cfg/a.json").exists());
        assert!(!wt.path.join("local/cfg/b.txt").exists());
        assert!(!wt.path.join(".env.other").exists());
        assert!(!wt.path.join("loose.txt").exists());
        assert_eq!(git(&*h, &wt.path, &["status", "--porcelain"]).unwrap(), "");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_snapshots_the_checkout_and_retires_its_name() {
        let h = h();
        let repo = temp_repo("snap");
        let wt = create(&*h, &repo, &req("snap-wt")).unwrap();
        std::fs::write(wt.path.join("a.txt"), "changed").unwrap();
        std::fs::write(wt.path.join("new.txt"), "untracked").unwrap();
        let m = managed(&*h, &wt.path).unwrap();
        assert!(remove(&*h, &m, false).unwrap_err().contains("uncommitted"));
        assert!(retired(&*h, &repo).is_empty());

        let removed = remove(&*h, &m, true).unwrap();
        assert_eq!(
            removed.snapshot.as_deref(),
            Some("refs/lyrel/trash/snap-wt")
        );
        assert!(!removed.branch_kept);
        assert!(!wt.path.exists());
        let show = |f: &str| {
            git(
                &*h,
                &repo,
                &["show", &format!("refs/lyrel/trash/snap-wt:{f}")],
            )
        };
        assert_eq!(show("a.txt").unwrap(), "changed");
        assert_eq!(show("new.txt").unwrap(), "untracked");
        assert_eq!(retired(&*h, &repo), ["snap-wt"]);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_reports_a_branch_it_had_to_keep() {
        let h = h();
        let repo = temp_repo("kept");
        let wt = create(&*h, &repo, &req("kept-wt")).unwrap();
        std::fs::write(wt.path.join("c.txt"), "c").unwrap();
        sh(&wt.path, &["git", "add", "."]);
        sh(&wt.path, &["git", "commit", "-q", "-m", "c"]);
        let removed = remove(&*h, &managed(&*h, &wt.path).unwrap(), false).unwrap();
        assert!(removed.branch_kept);
        assert!(branch_exists(&*h, &repo, "kept-wt"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn defaults_start_from_the_remote_default_branch_and_create_fetches_it() {
        let h = h();
        let upstream = temp_repo("up");
        let clone = scratch("clone");
        sh(
            clone.parent().unwrap(),
            &[
                "git",
                "clone",
                "-q",
                upstream.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        let d = defaults(&*h, &clone).unwrap();
        let expected = git(
            &*h,
            &clone,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )
        .unwrap();
        assert_eq!(d.base, expected);

        // Upstream moves on after the clone; the worktree still starts there.
        std::fs::write(upstream.join("later.txt"), "l").unwrap();
        sh(&upstream, &["git", "add", "."]);
        sh(&upstream, &["git", "commit", "-q", "-m", "later"]);
        let mut r = req("fresh-wt");
        r.base = d.base.clone();
        let wt = create(&*h, &clone, &r).unwrap();
        assert!(wt.path.join("later.txt").exists());
        assert!(git(&*h, &wt.path, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err());
        let _ = std::fs::remove_dir_all(&upstream);
        let _ = std::fs::remove_dir_all(&clone);
    }

    #[test]
    fn list_and_find_see_the_managed_checkouts() {
        let h = h();
        let repo = temp_repo("list");
        let wt = create(&*h, &repo, &req("list-wt")).unwrap();
        let listed = list(&*h, &wt.path).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].main && !listed[0].managed);
        assert!(!listed[1].main && listed[1].managed);
        assert_eq!(listed[1].branch.as_deref(), Some("list-wt"));
        assert_eq!(plain(&listed[1].path), plain(&wt.path));

        assert_eq!(find(&*h, &repo, "list-wt").unwrap().branch, "list-wt");
        assert_eq!(
            find(&*h, &repo, wt.path.to_str().unwrap()).unwrap().branch,
            "list-wt"
        );
        assert!(
            find(&*h, &repo, "nope")
                .unwrap_err()
                .contains("no worktree")
        );
        let tmp = repo.to_str().unwrap();
        assert!(
            find(&*h, &repo, tmp)
                .unwrap_err()
                .contains("not a worktree lyrel made")
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_replaces_the_legacy_catch_all_gitignore() {
        let h = h();
        let repo = temp_repo("legacy");
        std::fs::create_dir_all(repo.join(".lyrel")).unwrap();
        std::fs::write(repo.join(".lyrel/.gitignore"), "*\n").unwrap();
        std::fs::write(repo.join(".lyrel/setup"), "#!/bin/sh\n").unwrap();
        create(&*h, &repo, &req("legacy-wt")).unwrap();
        create(&*h, &repo, &req("legacy-wt2")).unwrap();
        assert!(!repo.join(".lyrel/.gitignore").exists());
        assert_eq!(
            git(&*h, &repo, &["status", "--porcelain"]).unwrap(),
            "?? .lyrel/"
        );
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(
            exclude
                .lines()
                .filter(|l| *l == "/.lyrel/worktrees/")
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// The Listed entry git reports for `path` — what a picker row names.
    fn listed_at(h: &dyn Host, repo: &Path, path: &Path) -> Listed {
        list(h, repo)
            .unwrap()
            .into_iter()
            .find(|wt| plain(&wt.path) == plain(path))
            .unwrap_or_else(|| panic!("no listed checkout at {}", path.display()))
    }

    fn add_external_worktree(repo: &Path, path: &Path, branch: &str) {
        sh(
            repo,
            &[
                "git",
                "worktree",
                "add",
                "-b",
                branch,
                path.to_str().unwrap(),
            ],
        );
    }

    #[test]
    fn remove_checkout_unregisters_an_external_clean_checkout_and_files_recovery() {
        let h = h();
        let repo = temp_repo("rmext");
        let wt = scratch("rmext-wt");
        let _ = std::fs::remove_dir_all(&wt);
        add_external_worktree(&repo, &wt, "ext-branch");
        let head = git(&*h, &wt, &["rev-parse", "HEAD"]).unwrap();
        let removed = remove_checkout(&*h, &listed_at(&*h, &repo, &wt)).unwrap();
        assert_eq!(removed.branch, BranchOutcome::Deleted);
        assert!(!wt.exists(), "the checkout directory is deleted");
        let plain_wt = plain(&wt);
        assert!(
            !list(&*h, &repo)
                .unwrap()
                .iter()
                .any(|listed| plain(&listed.path) == plain_wt),
            "git worktree list no longer registers it"
        );
        assert!(
            !branch_exists(&*h, &repo, "ext-branch"),
            "a fully merged branch is deleted"
        );
        assert_eq!(
            git(&*h, &repo, &["rev-parse", &removed.recovery]).unwrap(),
            head,
            "the recovery reference holds the checkout's last commit"
        );
        let name = wt.file_name().unwrap().to_str().unwrap();
        assert_eq!(removed.recovery, format!("{TRASH_REFS}/{name}"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_checkout_refuses_the_main_checkout() {
        let h = h();
        let repo = temp_repo("rmmain");
        let main = listed_at(&*h, &repo, &repo);
        assert!(main.main);
        let err = remove_checkout(&*h, &main).unwrap_err();
        assert!(err.contains("main checkout"), "{err}");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_checkout_refuses_a_dirty_checkout_and_touches_nothing() {
        let h = h();
        let repo = temp_repo("rmdirty");
        let wt = scratch("rmdirty-wt");
        let _ = std::fs::remove_dir_all(&wt);
        add_external_worktree(&repo, &wt, "dirty-branch");
        std::fs::write(wt.join("untracked.txt"), "dirty").unwrap();
        let err = remove_checkout(&*h, &listed_at(&*h, &repo, &wt)).unwrap_err();
        assert!(err.contains("uncommitted changes"), "{err}");
        assert!(wt.exists());
        assert_eq!(
            git(&*h, &repo, &["for-each-ref", "refs/lyrel/"]).unwrap(),
            "",
            "no recovery reference is created for a refused removal"
        );
        assert!(branch_exists(&*h, &repo, "dirty-branch"));
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn remove_checkout_deletes_a_merged_branch_and_keeps_an_unmerged_one() {
        let h = h();
        let repo = temp_repo("rmbr");
        let merged = scratch("rmbr-merged");
        let _ = std::fs::remove_dir_all(&merged);
        add_external_worktree(&repo, &merged, "merged-branch");
        let kept = scratch("rmbr-kept");
        let _ = std::fs::remove_dir_all(&kept);
        add_external_worktree(&repo, &kept, "unmerged-branch");
        std::fs::write(kept.join("own.txt"), "own").unwrap();
        sh(&kept, &["git", "add", "."]);
        sh(
            &kept,
            &[
                "git",
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "own",
            ],
        );
        let removed = remove_checkout(&*h, &listed_at(&*h, &repo, &merged)).unwrap();
        assert_eq!(removed.branch, BranchOutcome::Deleted);
        assert!(!branch_exists(&*h, &repo, "merged-branch"));
        let removed = remove_checkout(&*h, &listed_at(&*h, &repo, &kept)).unwrap();
        assert_eq!(removed.branch, BranchOutcome::Kept);
        assert!(
            branch_exists(&*h, &repo, "unmerged-branch"),
            "git branch -d refuses an unmerged branch and it stays"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_checkout_does_no_branch_work_on_a_detached_head() {
        let h = h();
        let repo = temp_repo("rmdetach");
        let wt = scratch("rmdetach-wt");
        let _ = std::fs::remove_dir_all(&wt);
        add_external_worktree(&repo, &wt, "detach-branch");
        sh(&wt, &["git", "checkout", "-q", "--detach", "HEAD"]);
        let listed = listed_at(&*h, &repo, &wt);
        assert_eq!(listed.branch, None, "the checkout reads detached");
        let before = git(&*h, &repo, &["for-each-ref", "refs/heads/"]).unwrap();
        let removed = remove_checkout(&*h, &listed).unwrap();
        assert_eq!(removed.branch, BranchOutcome::Detached);
        assert_eq!(
            git(&*h, &repo, &["for-each-ref", "refs/heads/"]).unwrap(),
            before,
            "no branch is deleted for a detached checkout"
        );
        assert!(
            branch_exists(&*h, &repo, "detach-branch"),
            "the branch the checkout was made on stays"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_checkout_never_overwrites_an_older_recovery_reference() {
        let h = h();
        let repo = temp_repo("rmref");
        let wt = scratch("rmref-wt");
        let _ = std::fs::remove_dir_all(&wt);
        add_external_worktree(&repo, &wt, "ref-branch");
        let older = git(&*h, &repo, &["rev-parse", "HEAD"]).unwrap();
        let name = wt.file_name().unwrap().to_str().unwrap();
        sh(
            &repo,
            &["git", "update-ref", &format!("{TRASH_REFS}/{name}"), &older],
        );
        let removed = remove_checkout(&*h, &listed_at(&*h, &repo, &wt)).unwrap();
        assert_eq!(removed.recovery, format!("{TRASH_REFS}/{name}-2"));
        assert_eq!(
            git(&*h, &repo, &["rev-parse", &format!("{TRASH_REFS}/{name}")]).unwrap(),
            older,
            "the older recovery reference still points where it did"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_checkout_picks_the_canonical_checkout_when_basenames_collide() {
        let h = h();
        let repo = temp_repo("rmbase");
        let a = scratch("rmbase-a").join("checkout");
        let b = scratch("rmbase-b").join("checkout");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        add_external_worktree(&repo, &a, "branch-a");
        add_external_worktree(&repo, &b, "branch-b");
        let head_a = git(&*h, &a, &["rev-parse", "HEAD"]).unwrap();
        let removed = remove_checkout(&*h, &listed_at(&*h, &repo, &a)).unwrap();
        assert_eq!(removed.recovery, format!("{TRASH_REFS}/checkout"));
        assert!(!a.exists());
        assert!(
            b.exists(),
            "the same-basename checkout that was not picked stays"
        );
        assert!(
            branch_exists(&*h, &repo, "branch-b"),
            "the surviving checkout's branch is untouched"
        );
        let removed = remove_checkout(&*h, &listed_at(&*h, &repo, &b)).unwrap();
        assert_eq!(removed.recovery, format!("{TRASH_REFS}/checkout-2"));
        assert_eq!(
            git(
                &*h,
                &repo,
                &["rev-parse", &format!("{TRASH_REFS}/checkout")]
            )
            .unwrap(),
            head_a,
            "the first recovery reference survived the second removal"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_checkout_fails_closed_when_no_recovery_name_is_free() {
        let h = h();
        let repo = temp_repo("rmfull");
        let wt = scratch("rmfull-wt");
        let _ = std::fs::remove_dir_all(&wt);
        add_external_worktree(&repo, &wt, "full-branch");
        let head = git(&*h, &repo, &["rev-parse", "HEAD"]).unwrap();
        let name = wt.file_name().unwrap().to_str().unwrap();
        // n in 1..100 tries <name> and <name>-2..<name>-99; take them all.
        for n in 1..100 {
            let refname = if n == 1 {
                format!("{TRASH_REFS}/{name}")
            } else {
                format!("{TRASH_REFS}/{name}-{n}")
            };
            sh(&repo, &["git", "update-ref", &refname, &head]);
        }
        let err = remove_checkout(&*h, &listed_at(&*h, &repo, &wt)).unwrap_err();
        assert!(err.contains("recovery reference"), "{err}");
        assert!(
            wt.exists(),
            "nothing is removed without a recovery reference"
        );
        assert!(
            branch_exists(&*h, &repo, "full-branch"),
            "no branch cleanup is attempted either"
        );
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn occupancy_is_tri_state_and_fails_closed() {
        let h = h();
        let repo = temp_repo("occ3");
        let wt = scratch("occ3-wt");
        let _ = std::fs::remove_dir_all(&wt);
        add_external_worktree(&repo, &wt, "occ3-branch");
        let deep = wt.join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        let outside = scratch("occ3-out");
        assert_eq!(
            occupancy(&*h, &wt, &[outside.clone()]),
            Ok(false),
            "a cwd outside the checkout proves it is free"
        );
        assert_eq!(
            occupancy(&*h, &wt, &[deep]),
            Ok(true),
            "a descendant cwd occupies the checkout"
        );
        assert!(
            occupancy(&*h, &wt, &[outside.join("gone")]).is_err(),
            "a cwd that cannot be canonicalized is not proof of absence"
        );
        assert!(
            occupancy(&*h, &wt.join("gone"), &[outside]).is_err(),
            "an unverifiable target blocks too"
        );
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&wt);
    }
}
