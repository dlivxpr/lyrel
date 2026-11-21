//! Captures dirty work on its owning Host without staging anything in the checkout.

use super::{CheckoutInspection, Host, Path, PathBuf, git};

const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const FSYNC: &str = "core.fsync=loose-object,reference";

type FileState = (String, Option<(String, String)>);

pub(super) struct Snapshot {
    commit: String,
    index_tree: String,
    files: Vec<FileState>,
}

impl Snapshot {
    pub(super) fn record(host: &dyn Host, checkout: &CheckoutInspection) -> Result<Self, String> {
        let index = PrivateIndex::new(host, checkout)?;
        let index_tree = index.run(&["write-tree"])?;
        let index_commit =
            index.commit(&index_tree, &[&checkout.head], "lyrel: staged recovery")?;
        let files = changed_files(host, checkout, true)?;
        // Remove vanished paths first: a deletion and an insertion can swap a
        // directory for a file, or vice versa.
        for (path, state) in &files {
            if state.is_none() {
                index.run(&["update-index", "--force-remove", "--", path])?;
            }
        }
        for (path, state) in &files {
            if let Some((mode, oid)) = state {
                index.run(&[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("{mode},{oid},{path}"),
                ])?;
            }
        }
        let tree = index.run(&["write-tree"])?;
        let commit = index.commit(
            &tree,
            &[&checkout.head, &index_commit],
            "lyrel: removed worktree recovery",
        )?;
        Ok(Self {
            commit,
            index_tree,
            files,
        })
    }

    pub(super) fn commit(&self) -> &str {
        &self.commit
    }

    pub(super) fn verify(
        &self,
        host: &dyn Host,
        checkout: &CheckoutInspection,
    ) -> Result<(), String> {
        let index = PrivateIndex::new(host, checkout)?;
        if index.run(&["write-tree"])? != self.index_tree
            || changed_files(host, checkout, false)? != self.files
        {
            return Err(
                "files or staged changes changed during recovery; the checkout was not removed"
                    .into(),
            );
        }
        Ok(())
    }
}

/// An independent Git index using the original object store. No process
/// environment overrides or local path assumptions are needed over SSH.
struct PrivateIndex<'a> {
    host: &'a dyn Host,
    checkout: &'a CheckoutInspection,
    dir: PathBuf,
}

impl<'a> PrivateIndex<'a> {
    fn new(host: &'a dyn Host, checkout: &'a CheckoutInspection) -> Result<Self, String> {
        let dir = host.join(
            &checkout.common,
            &format!("lyrel-index-{}", uuid::Uuid::new_v4()),
        );
        host.create_dir(&dir, false)
            .map_err(|e| format!("cannot create an independent index: {e}"))?;
        let index = Self {
            host,
            checkout,
            dir,
        };
        let root = as_utf8(&index.dir)?;
        let common = as_utf8(&checkout.common)?;
        if common.contains(['\r', '\n']) {
            return Err(
                "the repository path cannot be represented in a private Git directory".into(),
            );
        }
        git(
            host,
            &checkout.canonical,
            &["init", "--bare", "--quiet", "--template=", root],
        )?;
        host.write_file(
            &host.join(&index.dir, "commondir"),
            format!("{common}\n").as_bytes(),
        )
        .map_err(|e| format!("cannot connect the recovery index to this repository: {e}"))?;
        let source = host.join(&checkout.admin, "index");
        let bytes = host
            .read_file(&source, MAX_INDEX_BYTES)
            .map_err(|e| format!("cannot read the checkout's index: {e}"))?;
        host.write_file(&host.join(&index.dir, "index"), &bytes)
            .map_err(|e| format!("cannot write the recovery index: {e}"))?;
        Ok(index)
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let dir = format!("--git-dir={}", as_utf8(&self.dir)?);
        let root = format!("--work-tree={}", as_utf8(&self.checkout.canonical)?);
        let mut command = vec![
            dir.as_str(),
            root.as_str(),
            "-c",
            "core.bare=false",
            "-c",
            FSYNC,
        ];
        command.extend_from_slice(args);
        git(self.host, &self.checkout.canonical, &command)
    }

    fn commit(&self, tree: &str, parents: &[&str], message: &str) -> Result<String, String> {
        let mut args = vec![
            "-c",
            "user.name=lyrel",
            "-c",
            "user.email=lyrel@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            tree,
            "-m",
            message,
        ];
        for parent in parents {
            args.extend(["-p", parent]);
        }
        self.run(&args)
    }
}

impl Drop for PrivateIndex<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.host.remove(&self.dir, true) {
            log::warn!(
                "could not remove recovery index {}: {error}",
                self.dir.display()
            );
        }
    }
}

fn as_utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn output(host: &dyn Host, root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = host
        .git(root, args)
        .map_err(|e| format!("could not run Git: {e}"))?;
    if !out.success() {
        return Err(out.stderr_trimmed());
    }
    Ok(out.stdout)
}

fn changed_files(
    host: &dyn Host,
    checkout: &CheckoutInspection,
    write: bool,
) -> Result<Vec<FileState>, String> {
    let root = &checkout.canonical;
    let entries = output(host, root, &["ls-files", "-v", "--stage", "-z"])?;
    for entry in entries.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        let header = entry.split(|b| *b == b'\t').next().unwrap_or_default();
        if !header.starts_with(b"H ")
            || !header.ends_with(b" 0")
            || header.starts_with(b"H 160000 ")
        {
            return Err(
                "submodules, conflicts or hidden index entries cannot be recovered safely".into(),
            );
        }
    }

    let status = output(
        host,
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
            "--no-renames",
            "-z",
        ],
    )?;
    let mut files = Vec::new();
    for entry in status.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        let (code, path) = entry
            .split_at_checked(3)
            .ok_or("invalid Git status record")?;
        let path = std::str::from_utf8(path).map_err(|_| "a changed path is not valid UTF-8")?;
        if code == b"!! " {
            return Err(format!(
                "move ignored data out of the checkout before removal: {path}"
            ));
        }
        // The slash suffix indicates an embedded repository or directory Git
        // won't enumerate. Never follow a link out of the checkout to read it.
        if code[2] != b' '
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || (host.separator() == '\\' && path.contains(['\\', ':']))
        {
            return Err(format!("cannot capture this changed path safely: {path}"));
        }
        let absolute = host.join(root, path);
        let state = match host.stat(&absolute) {
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && (code[0] == b'D' || code[1] == b'D') =>
            {
                None
            }
            Err(e) => return Err(format!("could not verify {path}: {e}")),
            Ok(meta) => {
                let canonical = host
                    .canonicalize(&absolute)
                    .map_err(|e| format!("could not resolve {path}: {e}"))?;
                if meta.is_symlink || meta.is_dir || !canonical.starts_with(root) {
                    return Err(format!(
                        "cannot capture a modified link or directory safely: {path}"
                    ));
                }
                // Git reports the entry mode without running filters or textconv.
                let mode = host
                    .git(
                        root,
                        &[
                            "diff",
                            "--no-index",
                            "--raw",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--",
                            "/dev/null",
                            path,
                        ],
                    )
                    .map_err(|e| format!("cannot determine mode for {path}: {e}"))?;
                let mode = match (mode.status, mode.stdout.get(..14)) {
                    (Some(1), Some(b":000000 100644")) => "100644",
                    (Some(1), Some(b":000000 100755")) => "100755",
                    _ => return Err(format!("cannot capture a non-regular file: {path}")),
                };
                let mut args = vec!["-c", FSYNC, "hash-object", "--no-filters"];
                if write {
                    args.push("-w");
                }
                args.extend(["--", path]);
                let oid = git(host, root, &args)?;
                Some((mode.to_owned(), oid))
            }
        };
        files.push((path.to_owned(), state));
    }
    // `status` can emit the same replaced path in both index and untracked
    // sections. Only its final working-tree state belongs in the snapshot.
    files.sort_unstable();
    files.dedup();
    Ok(files)
}
