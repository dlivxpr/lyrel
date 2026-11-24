//! 内容感知 Diff 的桌面侧取数、身份与状态。
//!
//! 结构匹配器本身在 `lyrel-content-diff`（GUI-free，仅桌面生产依赖）。
//! 本模块负责它需要的另一半：按 [`DiffSource`] 解析被查看文件的两个版本，
//! 经所属 `Host` 读取有界字节，在后台线程完成解析与匹配，并保证异步结果
//! 只在来源、修订、Host 连接与代次都仍然匹配时落地。
//!
//! 与 `diff_probe` 的分工一致：`LyrelApp` 持有 [`ContentFetchTracker`] 并
//! 负责 registry 查找与 spawn，这里持有纯粹的请求/落地判定与数据模型。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lyrel_content_diff::{
    BlockKind, ContentError, EditKind, LineSpan, MAX_FILE_BYTES, StructuralDiff, language_for_path,
    structural_diff,
};
use lyrel_ui_base::host_ops::{HostId, SharedHost};
use lyrel_ui_base::i18n::L10nKey;

use crate::ui::scm::diff::{DiffLine, DiffSource, FileDiff, FileStatus, Hunk, LineKind};

/// One version of the file on the owning Host.
///
/// A side that does not exist — an added, deleted, or initial-commit side —
/// needs no variant of its own: git-addressed sides that fail to resolve read
/// as empty bytes, which is exactly how git itself defines them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VersionSpec {
    /// The index entry (`git show :path`).
    Index,
    /// A named revision (`git show rev:path`).
    Rev(String),
    /// The merge base of a three-dot range, resolved on the Host.
    MergeBase { base: String, head: String },
    /// The working tree (`Host::read_file`).
    Worktree,
}

/// Which two versions the viewed file's comparison reads, and under which
/// paths (a rename reads two different paths).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FetchPlan {
    pub(crate) old_path: String,
    pub(crate) new_path: String,
    pub(crate) old: VersionSpec,
    pub(crate) new: VersionSpec,
}

/// The sides a source compares for one file. `None` means the comparison has
/// no readable versions at all (a supplied patch), which the view explains.
///
/// The unstaged comparison is index → working tree, staged is HEAD → index,
/// combined is HEAD → working tree. A commit compares its first parent to the
/// commit; a three-dot range compares the merge base to its target.
pub(crate) fn plan_for(source: &DiffSource, old_path: &str, new_path: &str) -> Option<FetchPlan> {
    let (old, new) = match source {
        DiffSource::Worktree => (VersionSpec::Index, VersionSpec::Worktree),
        DiffSource::Staged => (VersionSpec::Rev("HEAD".into()), VersionSpec::Index),
        DiffSource::Head => (VersionSpec::Rev("HEAD".into()), VersionSpec::Worktree),
        DiffSource::Commit { rev, .. } => (
            VersionSpec::Rev(format!("{rev}^")),
            VersionSpec::Rev(rev.clone()),
        ),
        DiffSource::Range { base, head } => (
            VersionSpec::MergeBase {
                base: base.clone(),
                head: head.clone(),
            },
            VersionSpec::Rev(head.clone()),
        ),
        DiffSource::Patch { .. } => return None,
    };
    Some(FetchPlan {
        old_path: old_path.to_string(),
        new_path: new_path.to_string(),
        old,
        new,
    })
}

/// Why the content-aware view could not be produced. The view renders the
/// matching explanation and the ordinary line diff; the configured mode is
/// left alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ContentFailure {
    /// A supplied patch has no readable versions to compare.
    PatchOnly,
    /// No grammar for this file type in this build.
    Unsupported,
    /// One side looks binary.
    Binary,
    /// A byte, line, or item budget was exceeded.
    OverBudget,
    /// The text did not parse cleanly.
    ParseFailed,
    /// The Host could not supply the bytes.
    ReadFailed,
    /// The remote Host's server cannot send revision-addressed versions, so
    /// the bounded read has no route. This is a capability gap, not a read
    /// failure: the server needs updating.
    RemoteUnsupported,
}

impl ContentFailure {
    pub(crate) fn message(self) -> L10nKey {
        match self {
            ContentFailure::PatchOnly => L10nKey::DiffContentPatchOnly,
            ContentFailure::Unsupported => L10nKey::DiffContentUnsupported,
            ContentFailure::OverBudget => L10nKey::DiffContentOverBudget,
            ContentFailure::ParseFailed => L10nKey::DiffContentParseFailed,
            ContentFailure::Binary | ContentFailure::ReadFailed => L10nKey::DiffContentReadFailed,
            ContentFailure::RemoteUnsupported => L10nKey::DiffContentRemoteUnsupported,
        }
    }
}

/// One file's structural comparison, ready to draw.
pub(crate) struct ContentFile {
    /// The blocks' inner lines packaged as a patch file, so the row selection
    /// and copy paths work unchanged. One hunk per [`Item`](lyrel_content_diff::Item),
    /// in the same order, so a row id's `hunk` is the item index.
    pub(crate) file: Arc<FileDiff>,
    pub(crate) diff: Arc<StructuralDiff>,
}

/// The content-aware state of one file in one overlay.
#[derive(Clone)]
pub(crate) enum ContentState {
    Loading,
    Ready(Arc<ContentFile>),
    Failed(ContentFailure),
}

impl PartialEq for ContentState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ContentState::Loading, ContentState::Loading) => true,
            (ContentState::Ready(a), ContentState::Ready(b)) => {
                Arc::ptr_eq(a, b) || a.file == b.file
            }
            (ContentState::Failed(a), ContentState::Failed(b)) => a == b,
            _ => false,
        }
    }
}

/// The structured identity of one content fetch.
///
/// The epoch is the overlay's snapshot generation: a new snapshot invalidates
/// the bytes a fetch was reading, and a landing from the old generation must
/// not install over it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ContentFetchKey {
    pub(crate) host: HostId,
    pub(crate) cwd: PathBuf,
    pub(crate) source: DiffSource,
    pub(crate) path: String,
    pub(crate) epoch: u64,
}

/// What [`ContentFetchTracker::request`] decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ContentRequest {
    Spawn,
    /// The same host instance already has this exact read in flight.
    Coalesce,
}

/// What [`ContentFetchTracker::complete`] decided about one landing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ContentCompletion {
    Deliver,
    /// The key's in-flight entry belongs to a newer host's read, or is gone.
    Superseded,
    /// The reading host retired while the read was out.
    Retired,
}

/// Which content reads are out, and on which `SharedHost` instance.
///
/// The same `Arc::ptr_eq` discipline as [`DiffProbeTracker`](crate::ui::diff_probe::DiffProbeTracker):
/// a reconnect hands the same `HostId` to a new host while an old read may
/// still be out, and only instance identity separates "already reading" from
/// "a new connection that must read now, and a late landing it must never
/// overwrite".
#[derive(Default)]
pub(crate) struct ContentFetchTracker {
    inflight: HashMap<ContentFetchKey, SharedHost>,
    /// Keys invalidated while their read was out. Cleared when the follow-up
    /// read spawns.
    restale: HashSet<ContentFetchKey>,
}

impl ContentFetchTracker {
    pub(crate) fn request(&mut self, host: &SharedHost, key: &ContentFetchKey) -> ContentRequest {
        match self.inflight.get(key) {
            Some(active) if Arc::ptr_eq(active, host) => {
                self.restale.insert(key.clone());
                ContentRequest::Coalesce
            }
            Some(_) => {
                self.inflight.insert(key.clone(), Arc::clone(host));
                self.restale.remove(key);
                ContentRequest::Spawn
            }
            None => {
                self.inflight.insert(key.clone(), Arc::clone(host));
                ContentRequest::Spawn
            }
        }
    }

    /// `live` is the registry's current instance for the key's host id.
    pub(crate) fn complete(
        &mut self,
        key: &ContentFetchKey,
        probe: &SharedHost,
        live: Option<&SharedHost>,
    ) -> ContentCompletion {
        if !self
            .inflight
            .get(key)
            .is_some_and(|active| Arc::ptr_eq(active, probe))
        {
            return ContentCompletion::Superseded;
        }
        self.inflight.remove(key);
        let Some(current) = live.filter(|current| Arc::ptr_eq(current, probe)) else {
            self.restale.remove(key);
            return ContentCompletion::Retired;
        };
        let _ = current;
        self.restale.remove(key);
        ContentCompletion::Deliver
    }

    #[cfg(test)]
    pub(crate) fn inflight(&self) -> usize {
        self.inflight.len()
    }
}

/// Read one side's bytes on the Host. A git-addressed side that does not
/// exist — an added, deleted, or initial-commit side — reads as empty bytes,
/// exactly how git itself defines it; a repository-level failure is an
/// error, never an empty side. The working-tree side follows the same rule
/// for a deletion (`git diff` lists the file, but the path is gone), while
/// any other working-tree read failure is a real error.
fn read_side(
    host: &dyn lyrel_core::host::Host,
    root: &Path,
    path: &str,
    spec: &VersionSpec,
) -> Result<Vec<u8>, ContentFailure> {
    let limit = MAX_FILE_BYTES as u64;
    match spec {
        // The path is on the Host, whose separator need not be this
        // process's: a Linux checkout read from a Windows desktop is still
        // `/home/me/repo`. `Path::join` would spell it with `\\` before the
        // wire ever saw it.
        VersionSpec::Worktree => match host.read_file(&host.join(root, path), limit) {
            Ok(bytes) => Ok(bytes),
            // A deleted file's new side is empty, exactly as git defines a
            // deletion; the snapshot still lists it so the block can show
            // what left the tree.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(match error.kind() {
                std::io::ErrorKind::FileTooLarge => ContentFailure::OverBudget,
                std::io::ErrorKind::Unsupported => ContentFailure::RemoteUnsupported,
                _ => ContentFailure::ReadFailed,
            }),
        },
        VersionSpec::Index => read_git_side(host, root, &format!(":{path}")),
        VersionSpec::Rev(rev) => read_git_side(host, root, &format!("{rev}:{path}")),
        VersionSpec::MergeBase { base, head } => {
            let out = host
                .git(root, &["merge-base", base, head])
                .map_err(|_| ContentFailure::ReadFailed)?;
            if !out.success() {
                return Err(ContentFailure::ReadFailed);
            }
            let merge_base = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if merge_base.is_empty() {
                return Err(ContentFailure::ReadFailed);
            }
            read_git_side(host, root, &format!("{merge_base}:{path}"))
        }
    }
}

/// One revision-addressed side, bounded on the owning Host. `Ok(None)` means
/// the side does not exist in that revision, which Git defines as empty.
fn read_git_side(
    host: &dyn lyrel_core::host::Host,
    root: &Path,
    spec: &str,
) -> Result<Vec<u8>, ContentFailure> {
    match host.git_blob(root, spec, MAX_FILE_BYTES as u64) {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Ok(Vec::new()),
        Err(error) => Err(git_read_failure(error)),
    }
}

/// What a failed bounded version read means to the view. A transport without
/// the capability is its own reason; a budget refusal is the existing
/// over-budget fallback; everything else is an ordinary read failure.
fn git_read_failure(error: std::io::Error) -> ContentFailure {
    match error.kind() {
        std::io::ErrorKind::Unsupported => ContentFailure::RemoteUnsupported,
        std::io::ErrorKind::FileTooLarge => ContentFailure::OverBudget,
        _ => ContentFailure::ReadFailed,
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// Fetch, decode, parse and match one file. Runs on the Host executor.
pub(crate) fn fetch_content(
    host: &dyn lyrel_core::host::Host,
    root: &Path,
    plan: &FetchPlan,
    status: FileStatus,
) -> Result<ContentFile, ContentFailure> {
    let old_bytes = read_side(host, root, &plan.old_path, &plan.old)?;
    let new_bytes = read_side(host, root, &plan.new_path, &plan.new)?;
    if is_binary(&old_bytes) || is_binary(&new_bytes) {
        return Err(ContentFailure::Binary);
    }
    let old_text = String::from_utf8_lossy(&old_bytes).into_owned();
    let new_text = String::from_utf8_lossy(&new_bytes).into_owned();

    let language = language_for_path(&plan.new_path)
        .or_else(|| language_for_path(&plan.old_path))
        .ok_or(ContentFailure::Unsupported)?;
    let diff = structural_diff(language, &old_text, &new_text).map_err(|error| match error {
        ContentError::Unsupported => ContentFailure::Unsupported,
        ContentError::Binary => ContentFailure::Binary,
        ContentError::OverBudget(_) => ContentFailure::OverBudget,
        ContentError::ParseFailed => ContentFailure::ParseFailed,
    })?;

    let file = synthesize_file(&plan.new_path, status, &diff, &old_text, &new_text);
    Ok(ContentFile {
        file: Arc::new(file),
        diff: Arc::new(diff),
    })
}

/// Package the structural blocks as a patch file: one hunk per display item,
/// holding the block's inner lines in git order (removals before additions,
/// context between). Header rows and source anchors are drawn by the content
/// row builder from the structural model itself; their hunks are empty.
fn synthesize_file(
    path: &str,
    status: FileStatus,
    diff: &StructuralDiff,
    old_text: &str,
    new_text: &str,
) -> FileDiff {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let line_text = |lines: &[&str], no: u32| -> String {
        lines
            .get(no.saturating_sub(1) as usize)
            .copied()
            .unwrap_or_default()
            .to_string()
    };

    let mut added = 0u32;
    let mut removed = 0u32;
    let mut hunks = Vec::with_capacity(diff.items.len());
    for item in &diff.items {
        let (old_span, new_span) = match item {
            lyrel_content_diff::Item::Block(block) => (block.old, block.new),
            lyrel_content_diff::Item::Source { old, .. } => (Some(*old), None),
        };
        let lines: Vec<DiffLine> = match item {
            lyrel_content_diff::Item::Block(block) => block
                .edits
                .iter()
                .map(|edit| {
                    let (kind, text) = match edit.kind {
                        EditKind::Context => (
                            LineKind::Context,
                            line_text(&new_lines, edit.new.unwrap_or(1)),
                        ),
                        EditKind::Removed => {
                            removed += 1;
                            (
                                LineKind::Removed,
                                line_text(&old_lines, edit.old.unwrap_or(1)),
                            )
                        }
                        EditKind::Added => {
                            added += 1;
                            (
                                LineKind::Added,
                                line_text(&new_lines, edit.new.unwrap_or(1)),
                            )
                        }
                    };
                    DiffLine {
                        kind,
                        old_no: edit.old,
                        new_no: edit.new,
                        text,
                    }
                })
                .collect(),
            lyrel_content_diff::Item::Source { .. } => Vec::new(),
        };
        let header = match (old_span, new_span) {
            (Some(old), Some(new)) => format!("@@ {} → {} @@", span_text(old), span_text(new)),
            (None, Some(new)) => format!("@@ +{} @@", span_text(new)),
            (Some(old), None) => format!("@@ -{} @@", span_text(old)),
            (None, None) => String::new(),
        };
        hunks.push(Hunk { header, lines });
    }
    FileDiff {
        path: path.to_string(),
        old_path: None,
        status,
        added,
        removed,
        binary: false,
        truncated: None,
        hunks,
    }
}

fn span_text(span: LineSpan) -> String {
    match span.start == span.end {
        true => span.start.to_string(),
        false => format!("{}–{}", span.start, span.end),
    }
}

/// The block-kind word for a header row.
pub(crate) fn kind_label(kind: BlockKind) -> L10nKey {
    match kind {
        BlockKind::Moved | BlockKind::MovedEdited => L10nKey::DiffContentMoved,
        BlockKind::Modified => L10nKey::DiffContentModified,
        BlockKind::Added => L10nKey::DiffContentAdded,
        BlockKind::Deleted => L10nKey::DiffContentDeleted,
    }
}

/// The item-family word for a block header, localized.
pub(crate) fn item_label(label: &str) -> L10nKey {
    match label {
        "function" => L10nKey::DiffContentItemFunction,
        "method" => L10nKey::DiffContentItemMethod,
        "class" => L10nKey::DiffContentItemClass,
        "module" => L10nKey::DiffContentItemModule,
        "struct" => L10nKey::DiffContentItemStruct,
        "enum" => L10nKey::DiffContentItemEnum,
        "trait" => L10nKey::DiffContentItemTrait,
        "impl" => L10nKey::DiffContentItemImpl,
        "type" => L10nKey::DiffContentItemType,
        "constant" => L10nKey::DiffContentItemConstant,
        "macro" => L10nKey::DiffContentItemMacro,
        "key" => L10nKey::DiffContentItemKey,
        "section" => L10nKey::DiffContentItemSection,
        "import" => L10nKey::DiffContentItemImport,
        "variable" => L10nKey::DiffContentItemVariable,
        "array" => L10nKey::DiffContentItemArray,
        "sequence" => L10nKey::DiffContentItemSequence,
        "comment" => L10nKey::DiffContentItemComment,
        _ => L10nKey::DiffContentItemItem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_plan(source: DiffSource) -> Option<FetchPlan> {
        plan_for(&source, "old.rs", "new.rs")
    }

    #[test]
    fn each_source_reads_the_versions_git_defines() {
        let plan = source_plan(DiffSource::Worktree).unwrap();
        assert_eq!(plan.old, VersionSpec::Index);
        assert_eq!(plan.new, VersionSpec::Worktree);

        let plan = source_plan(DiffSource::Staged).unwrap();
        assert_eq!(plan.old, VersionSpec::Rev("HEAD".into()));
        assert_eq!(plan.new, VersionSpec::Index);

        let plan = source_plan(DiffSource::Head).unwrap();
        assert_eq!(plan.old, VersionSpec::Rev("HEAD".into()));
        assert_eq!(plan.new, VersionSpec::Worktree);

        let plan = source_plan(DiffSource::commit("abc")).unwrap();
        assert_eq!(plan.old, VersionSpec::Rev("abc^".into()));
        assert_eq!(plan.new, VersionSpec::Rev("abc".into()));

        let plan = source_plan(DiffSource::Range {
            base: "main".into(),
            head: "feature".into(),
        })
        .unwrap();
        assert_eq!(
            plan.old,
            VersionSpec::MergeBase {
                base: "main".into(),
                head: "feature".into()
            }
        );
        assert_eq!(plan.new, VersionSpec::Rev("feature".into()));

        assert!(
            source_plan(DiffSource::Patch {
                id: "o/r#1".into(),
                label: None
            })
            .is_none()
        );
    }

    #[test]
    fn renames_read_the_two_real_paths() {
        let plan = plan_for(&DiffSource::Worktree, "old/name.rs", "new/name.rs").unwrap();
        assert_eq!(plan.old_path, "old/name.rs");
        assert_eq!(plan.new_path, "new/name.rs");
    }

    #[test]
    fn a_landing_from_a_retired_instance_is_rejected() {
        let mut tracker = ContentFetchTracker::default();
        let retired = lyrel_core::host::local::new();
        let live = lyrel_core::host::local::new();
        assert_eq!(retired.id(), live.id());
        assert!(!Arc::ptr_eq(&retired, &live));
        let key = ContentFetchKey {
            host: retired.id(),
            cwd: PathBuf::from("/repo"),
            source: DiffSource::Worktree,
            path: "a.rs".into(),
            epoch: 3,
        };
        assert_eq!(tracker.request(&retired, &key), ContentRequest::Spawn);
        assert_eq!(tracker.request(&retired, &key), ContentRequest::Coalesce);
        // A reconnect's request supersedes the retired read.
        assert_eq!(tracker.request(&live, &key), ContentRequest::Spawn);
        assert_eq!(
            tracker.complete(&key, &retired, Some(&live)),
            ContentCompletion::Superseded
        );
        assert_eq!(
            tracker.complete(&key, &live, Some(&live)),
            ContentCompletion::Deliver
        );
        assert_eq!(tracker.inflight(), 0);
    }

    #[test]
    fn a_landing_after_the_host_left_is_retired() {
        let mut tracker = ContentFetchTracker::default();
        let host = lyrel_core::host::local::shared();
        let key = ContentFetchKey {
            host: host.id(),
            cwd: PathBuf::from("/repo"),
            source: DiffSource::Head,
            path: "a.rs".into(),
            epoch: 0,
        };
        assert_eq!(tracker.request(&host, &key), ContentRequest::Spawn);
        assert_eq!(
            tracker.complete(&key, &host, None),
            ContentCompletion::Retired
        );
        assert_eq!(tracker.inflight(), 0);
    }

    #[test]
    fn the_epoch_is_part_of_the_read_identity() {
        let a = ContentFetchKey {
            host: HostId::LOCAL,
            cwd: PathBuf::from("/repo"),
            source: DiffSource::Worktree,
            path: "a.rs".into(),
            epoch: 1,
        };
        let b = ContentFetchKey {
            epoch: 2,
            ..a.clone()
        };
        assert_ne!(a, b, "a new snapshot invalidates the bytes being read");
        let c = ContentFetchKey {
            path: "b.rs".into(),
            ..a.clone()
        };
        assert_ne!(a, c);
        let d = ContentFetchKey {
            source: DiffSource::Staged,
            ..a.clone()
        };
        assert_ne!(a, d);
        // Two commits are two revisions however much they share; a result
        // for one can never fill the other.
        let first = ContentFetchKey {
            source: DiffSource::commit("aaaa"),
            ..a.clone()
        };
        let second = ContentFetchKey {
            source: DiffSource::commit("bbbb"),
            ..a
        };
        assert_ne!(first, second);
    }

    #[test]
    fn a_failed_version_read_keeps_the_reason_it_found() {
        assert_eq!(
            git_read_failure(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "the peer cannot send one version"
            )),
            ContentFailure::RemoteUnsupported,
            "a capability gap is not an ordinary read failure"
        );
        assert_eq!(
            git_read_failure(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "over the budget"
            )),
            ContentFailure::OverBudget
        );
        assert_eq!(
            git_read_failure(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "link down"
            )),
            ContentFailure::ReadFailed
        );
    }

    #[test]
    fn failure_messages_name_their_reason() {
        assert_eq!(
            ContentFailure::PatchOnly.message(),
            L10nKey::DiffContentPatchOnly
        );
        assert_eq!(
            ContentFailure::Unsupported.message(),
            L10nKey::DiffContentUnsupported
        );
        assert_eq!(
            ContentFailure::OverBudget.message(),
            L10nKey::DiffContentOverBudget
        );
        assert_eq!(
            ContentFailure::ParseFailed.message(),
            L10nKey::DiffContentParseFailed
        );
        assert_eq!(
            ContentFailure::RemoteUnsupported.message(),
            L10nKey::DiffContentRemoteUnsupported
        );
        assert_eq!(
            ContentFailure::Binary.message(),
            L10nKey::DiffContentReadFailed
        );
        assert_eq!(
            ContentFailure::ReadFailed.message(),
            L10nKey::DiffContentReadFailed
        );
    }
}
