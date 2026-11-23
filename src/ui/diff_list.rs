//! Flattening a diff snapshot into the one-dimensional list of rows the
//! overlay scrolls.
//!
//! The overlay used to build its whole patch as a nested element tree —
//! a card per file, a hunk header and one element per line inside it, every
//! one of them constructed on every frame. A `20_000` line budget is around
//! `120_000` elements to lay out, and gpui notifies the view on each scroll
//! wheel event, so a diff of any size rebuilt the entire tree tens of times a
//! second.
//!
//! [`gpui::list`] only builds the rows it can see, but it is one-dimensional:
//! it takes an index, not a tree. So the tree is flattened here, once per
//! change to what is on screen.

use std::collections::{HashMap, HashSet};

use gpui_kit::SharedString;

use crate::ui::content_diff::{ContentFailure, ContentFile, ContentState};
use crate::ui::diff_rows::{RowId, SplitRow, UnifiedRow, split_hunk, unified_rows};
use crate::ui::scm::diff::{
    AUTO_COLLAPSE_LINES, DiffLine, DiffSnapshot, FileDiff, FileStatus, LineKind, MAX_RENDERED_FILES,
    Truncation,
};
use lyrel_content_diff::{BlockKind, Item, LineSpan};
use lyrel_core::core::config::DiffViewMode;
use lyrel_ui_base::i18n::L10nKey;

/// Everything the file header row draws, lifted out of its [`FileDiff`].
///
/// A copy rather than a borrow: the row list outlives the frame that built it,
/// and these are a handful of small fields against a file's whole patch.
#[derive(PartialEq, Eq)]
pub(crate) struct FileHead {
    /// Position in `snap.files`, for the element id and nothing else.
    pub(crate) index: usize,
    pub(crate) path: String,
    /// `old → new` for a rename, the path itself otherwise.
    pub(crate) shown_path: String,
    pub(crate) status: FileStatus,
    pub(crate) added: u32,
    pub(crate) removed: u32,
    pub(crate) binary: bool,
    pub(crate) expandable: bool,
    pub(crate) expanded: bool,
}

/// Where a drawn line sits in the patch: which file's rows it belongs to, and
/// its place among them.
///
/// Carried on the row rather than read off its position in the list. The list
/// is spliced — collapsing a file above this one moves every index below it —
/// so a range keyed on list positions would go on naming the same slots while
/// the code under them changed.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RowAt {
    /// One of these rides on every line of a patch, so the path is shared
    /// rather than copied per row.
    pub(crate) path: SharedString,
    pub(crate) id: RowId,
}

#[derive(PartialEq, Eq)]
pub(crate) enum DiffRow {
    /// The space that used to be the `gap_3` of a flex column.
    Gap,
    /// The banner above an auto-collapsed tree. Its text is derived from the
    /// snapshot at render time, so nothing is carried here.
    Oversized,
    FileHeader(FileHead),
    HunkHeader {
        text: String,
        /// The first hunk of a file follows its header and needs no rule
        /// above it; every later one is separating itself from the lines of
        /// the hunk before.
        leads: bool,
    },
    Split {
        row: SplitRow,
        at: RowAt,
    },
    Unified {
        row: UnifiedRow,
        at: RowAt,
    },
    Truncated(Truncation),
    MoreFiles {
        rest: usize,
    },
    UntrackedHeader {
        total: usize,
    },
    Untracked {
        index: usize,
        path: String,
    },
    MoreUntracked {
        rest: usize,
    },
    /// A content-aware block header: what the block is, where it was, where
    /// it is, and how to reach the other side.
    ContentBlock(ContentHead),
    /// An old-side position a block moved away from, jumpable to the block.
    ContentAnchor(ContentAnchorHead),
    /// A run of unchanged block lines the content view folded away.
    ContentElided {
        count: usize,
    },
    /// One line of a content-aware block's inner diff, in either column.
    ContentSplit {
        row: SplitRow,
        at: RowAt,
    },
    /// One line of a content-aware block's inner diff, single-column.
    ContentUnified {
        row: UnifiedRow,
        at: RowAt,
    },
    /// An aside in the content card: reading, or why the line diff is
    /// standing in for the structural view.
    ContentStatus(L10nKey),
}

/// Row id fields for the two non-line rows of a content item. Both live on
/// the item's own hunk index; their `row` values are outside any real line
/// index so a jump target can never collide with a selectable line.
pub(crate) const CONTENT_HEADER_ROW: usize = usize::MAX;
pub(crate) const CONTENT_ANCHOR_ROW: usize = usize::MAX - 1;

/// Everything a content-aware block header draws.
#[derive(PartialEq, Eq)]
pub(crate) struct ContentHead {
    /// Position in the file's structural items; the hunk index of its lines.
    pub(crate) index: usize,
    pub(crate) path: SharedString,
    pub(crate) kind: BlockKind,
    pub(crate) label: &'static str,
    pub(crate) name: Option<String>,
    pub(crate) old: Option<LineSpan>,
    pub(crate) new: Option<LineSpan>,
    /// Only formatting differs; the header says so instead of drawing lines.
    pub(crate) formatting_only: bool,
    /// How many inner rows follow; zero means a header-only block.
    pub(crate) edits: usize,
    /// The source anchor to jump to, for a block that left its old position.
    pub(crate) jump: Option<RowId>,
    /// The first row of the file's content sequence.
    pub(crate) leads: bool,
}

/// An old-side anchor row for a block that moved away.
#[derive(PartialEq, Eq)]
pub(crate) struct ContentAnchorHead {
    pub(crate) index: usize,
    pub(crate) path: SharedString,
    pub(crate) label: &'static str,
    pub(crate) name: Option<String>,
    pub(crate) old: LineSpan,
    pub(crate) new_start: u32,
    /// The block header this anchor jumps to.
    pub(crate) jump: RowId,
}

/// Which rows a file's content-aware card draws.
///
/// Bundled so the many pure row tests can pass [`ContentViews::default`] and
/// keep asserting the line views.
pub(crate) struct ContentViews<'a> {
    /// Per-file structural state, keyed by path.
    pub(crate) states: &'a HashMap<String, ContentState>,
    /// The layout the blocks' inner lines use: `Split` when the surface is
    /// wide enough for a paired view, `Unified` in a narrow dock.
    pub(crate) inner: DiffViewMode,
}

impl Default for ContentViews<'_> {
    fn default() -> Self {
        static NONE: std::sync::LazyLock<HashMap<String, ContentState>> =
            std::sync::LazyLock::new(HashMap::new);
        ContentViews {
            states: &NONE,
            inner: DiffViewMode::Split,
        }
    }
}

/// The rows of the whole overlay, in the order they scroll past.
pub(crate) fn build_rows(
    snap: &DiffSnapshot,
    expanded: &HashMap<String, bool>,
    focused: Option<usize>,
    mode: DiffViewMode,
    oversized: bool,
    content: &ContentViews<'_>,
) -> Vec<DiffRow> {
    let mut out: Vec<DiffRow> = Vec::new();
    // Stands in for the gap between the groups this list used to be a flex
    // column of: gpui's list stacks its items with nothing between them.
    let gap = |out: &mut Vec<DiffRow>| {
        if !out.is_empty() {
            out.push(DiffRow::Gap);
        }
    };

    if oversized {
        out.push(DiffRow::Oversized);
    }

    let shown = snap.files.len().min(MAX_RENDERED_FILES);
    for (idx, file) in snap.files.iter().enumerate() {
        if focused.is_some_and(|f| f != idx) {
            continue;
        }
        if focused.is_none() && idx >= shown {
            break;
        }
        let is_expanded = if focused == Some(idx) {
            expanded.get(&file.path).copied().unwrap_or(true)
        } else {
            file_expanded(file, expanded, oversized)
        };
        gap(&mut out);
        out.extend(file_rows(idx, file, is_expanded, mode, content));
    }

    if focused.is_none() && snap.files.len() > shown {
        gap(&mut out);
        out.push(DiffRow::MoreFiles {
            rest: snap.files.len() - shown,
        });
    }

    if focused.is_none() && !snap.untracked.is_empty() {
        gap(&mut out);
        out.extend(untracked_rows(snap));
    }

    out
}

/// The single synthesized file an untracked file's preview shows.
///
/// `usize::MAX` keeps its element ids clear of the real list's.
pub(crate) fn preview_rows(file: &FileDiff, mode: DiffViewMode) -> Vec<DiffRow> {
    file_rows(usize::MAX, file, true, mode, &ContentViews::default())
}

/// The content-aware card for a focused untracked file: the file header plus
/// its structural blocks. `usize::MAX` keeps its element ids clear of the
/// real list's, as [`preview_rows`] does.
pub(crate) fn content_preview_rows(
    path: &str,
    state: &ContentState,
    inner: DiffViewMode,
) -> Vec<DiffRow> {
    let (added, removed) = match state {
        ContentState::Ready(cf) => (cf.file.added, cf.file.removed),
        _ => (0, 0),
    };
    // An untracked binary file has no textual comparison; the header's
    // binary badge is the existing indication, exactly as in the line view.
    let binary = matches!(state, ContentState::Failed(ContentFailure::Binary));
    let mut rows = vec![DiffRow::FileHeader(FileHead {
        index: usize::MAX,
        path: path.to_string(),
        shown_path: path.to_string(),
        status: FileStatus::Added,
        added,
        removed,
        binary,
        expandable: false,
        expanded: true,
    })];
    match state {
        ContentState::Ready(cf) => {
            rows.extend(content_rows(usize::MAX, path, cf, inner));
        }
        ContentState::Failed(ContentFailure::Binary) => {}
        ContentState::Failed(reason) => {
            rows.push(DiffRow::ContentStatus(reason.message()));
        }
        ContentState::Loading => {
            rows.push(DiffRow::ContentStatus(L10nKey::DiffContentReading));
        }
    }
    rows
}

/// The view `file` draws in: a new or deleted file has one side only, so
/// split would be half empty.
pub(crate) fn file_mode(file: &FileDiff, mode: DiffViewMode) -> DiffViewMode {
    match file.status {
        FileStatus::Added | FileStatus::Deleted => DiffViewMode::Unified,
        _ => mode,
    }
}

fn file_rows(
    index: usize,
    file: &FileDiff,
    expanded: bool,
    mode: DiffViewMode,
    content: &ContentViews<'_>,
) -> Vec<DiffRow> {
    let expandable =
        !file.binary && (!file.hunks.is_empty() || file.truncated == Some(Truncation::Budget));
    let shown_path = match &file.old_path {
        Some(old) => format!("{old} → {}", file.path),
        None => file.path.clone(),
    };
    let mut rows = vec![DiffRow::FileHeader(FileHead {
        index,
        path: file.path.clone(),
        shown_path,
        status: file.status,
        added: file.added,
        removed: file.removed,
        binary: file.binary,
        expandable,
        expanded,
    })];

    if expanded && mode == DiffViewMode::Content {
        rows.extend(content_file_rows(index, file, content));
        return rows;
    }

    let mode = file_mode(file, mode);
    if expanded && (!file.hunks.is_empty() || file.truncated.is_some()) {
        rows.extend(hunk_rows(file, mode));
    }

    rows
}

/// The ordinary line rows of one file: a header per hunk and its lines.
fn hunk_rows(file: &FileDiff, mode: DiffViewMode) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let path = SharedString::from(file.path.clone());
    for (h, hunk) in file.hunks.iter().enumerate() {
        rows.push(DiffRow::HunkHeader {
            text: hunk.header.clone(),
            leads: h == 0,
        });
        let at = |row| RowAt {
            path: path.clone(),
            id: RowId { hunk: h, row },
        };
        match mode {
            DiffViewMode::Split | DiffViewMode::Content => rows.extend(
                split_hunk(&hunk.lines)
                    .into_iter()
                    .enumerate()
                    .map(|(r, row)| DiffRow::Split { row, at: at(r) }),
            ),
            DiffViewMode::Unified => rows.extend(
                unified_rows(&hunk.lines)
                    .into_iter()
                    .enumerate()
                    .map(|(r, row)| DiffRow::Unified { row, at: at(r) }),
            ),
        }
    }
    if let Some(reason) = file.truncated {
        rows.push(DiffRow::Truncated(reason));
    }
    rows
}

/// One file's content-aware card: the block sequence, or the explanation and
/// the ordinary line diff when the structural view cannot be produced. The
/// configured mode is never changed by a fallback.
fn content_file_rows(
    index: usize,
    file: &FileDiff,
    content: &ContentViews<'_>,
) -> Vec<DiffRow> {
    // A binary file has no textual versions to compare, and no content read
    // is dispatched for it: the file header's binary indication is the whole
    // card, exactly as in the line views. Without this the card would claim
    // to be reading bytes that were never asked for.
    if file.binary {
        return Vec::new();
    }
    let inner = match content.inner {
        DiffViewMode::Unified => DiffViewMode::Unified,
        _ => DiffViewMode::Split,
    };
    match content.states.get(&file.path) {
        Some(ContentState::Ready(cf)) => content_rows(index, &file.path, cf, inner),
        Some(ContentState::Failed(reason)) => {
            let mut rows = vec![DiffRow::ContentStatus(reason.message())];
            rows.extend(hunk_rows(file, DiffViewMode::Split));
            rows
        }
        Some(ContentState::Loading) | None => {
            vec![DiffRow::ContentStatus(L10nKey::DiffContentReading)]
        }
    }
}

/// The block sequence of one file's structural comparison.
fn content_rows(
    index: usize,
    path: &str,
    cf: &ContentFile,
    inner: DiffViewMode,
) -> Vec<DiffRow> {
    let _ = index;
    let path = SharedString::from(path.to_string());
    let mut rows: Vec<DiffRow> = Vec::new();
    for (item_ix, item) in cf.diff.items.iter().enumerate() {
        match item {
            Item::Source {
                label,
                name,
                old,
                new_start,
            } => rows.push(DiffRow::ContentAnchor(ContentAnchorHead {
                index: item_ix,
                path: path.clone(),
                label,
                name: name.clone(),
                old: *old,
                new_start: *new_start,
                jump: RowId {
                    hunk: item_ix,
                    row: CONTENT_HEADER_ROW,
                },
            })),
            Item::Block(block) => {
                let moved = matches!(
                    block.kind,
                    BlockKind::Moved | BlockKind::MovedEdited
                );
                rows.push(DiffRow::ContentBlock(ContentHead {
                    index: item_ix,
                    path: path.clone(),
                    kind: block.kind,
                    label: block.label,
                    name: block.name.clone(),
                    old: block.old,
                    new: block.new,
                    formatting_only: block.formatting_only,
                    edits: block.edits.len(),
                    jump: moved.then_some(RowId {
                        hunk: item_ix,
                        row: CONTENT_ANCHOR_ROW,
                    }),
                    leads: rows.is_empty(),
                }));
                if let Some(hunk) = cf.file.hunks.get(item_ix) {
                    if !hunk.lines.is_empty() {
                        rows.extend(content_line_rows(
                            &hunk.lines,
                            item_ix,
                            &path,
                            inner,
                        ));
                    }
                }
            }
        }
    }
    rows
}

/// How many unchanged lines the content view keeps at each end of a folded
/// run — the same three lines of context git shows around a hunk.
const CONTENT_CONTEXT: usize = 3;

/// One block's inner rows, with long unchanged runs folded into a labelled
/// marker. Row ids keep pointing at the hunk's own line indices, so copy
/// reads the file's text rather than anything this fold invented.
fn content_line_rows(
    lines: &[DiffLine],
    item_ix: usize,
    path: &SharedString,
    inner: DiffViewMode,
) -> Vec<DiffRow> {
    let elided = elided_context(lines);
    let at = |row| RowAt {
        path: path.clone(),
        id: RowId { hunk: item_ix, row },
    };
    // Build the rows from the whole line list first: the pairing between
    // removals and additions (and the `line` index each cell carries) is
    // computed over the block as it really is, and the fold only removes
    // whole unchanged rows afterwards.
    let built: Vec<DiffRow> = match inner {
        DiffViewMode::Unified => unified_rows(lines)
            .into_iter()
            .enumerate()
            .map(|(r, row)| DiffRow::ContentUnified { row, at: at(r) })
            .collect(),
        _ => split_hunk(lines)
            .into_iter()
            .enumerate()
            .map(|(r, row)| DiffRow::ContentSplit { row, at: at(r) })
            .collect(),
    };
    let mut rows = Vec::with_capacity(built.len());
    let mut pending = 0usize;
    let mut kept_any = false;
    for row in built {
        let hidden = match &row {
            DiffRow::ContentSplit { row, .. } => {
                let line = row
                    .left
                    .as_ref()
                    .filter(|cell| !cell.changed)
                    .map(|cell| cell.line);
                line.is_some_and(|line| elided.contains(&line))
                    && row
                        .right
                        .as_ref()
                        .is_none_or(|cell| !cell.changed)
            }
            DiffRow::ContentUnified { row, .. } => {
                row.kind == LineKind::Context && elided.contains(&row.line)
            }
            _ => false,
        };
        if hidden {
            pending += 1;
            continue;
        }
        // A marker only between drawn rows: the block header already names
        // the span, so leading and trailing folds need no note of their own.
        if pending > 0 && kept_any {
            rows.push(DiffRow::ContentElided { count: pending });
        }
        pending = 0;
        kept_any = true;
        rows.push(row);
    }
    rows
}

/// The context line indices a block's fold removes: the middle of every
/// unchanged run longer than twice [`CONTENT_CONTEXT`], and all but the last
/// (or first) few lines of a run that touches the block's start (or end).
fn elided_context(lines: &[DiffLine]) -> HashSet<usize> {
    let mut elided = HashSet::new();
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind != LineKind::Context)
        .map(|(ix, _)| ix)
        .collect();
    let (Some(first), Some(last)) = (changed.first(), changed.last()) else {
        return elided;
    };
    // Leading context: keep the last `CONTENT_CONTEXT` lines before the first
    // change.
    for ix in 0..first.saturating_sub(CONTENT_CONTEXT) {
        elided.insert(ix);
    }
    // Trailing context: keep the first `CONTENT_CONTEXT` after the last.
    for ix in (last + 1 + CONTENT_CONTEXT)..lines.len() {
        elided.insert(ix);
    }
    // Runs between two changes.
    for window in changed.windows(2) {
        let (from, to) = (window[0] + 1, window[1]);
        if to.saturating_sub(from) > CONTENT_CONTEXT * 2 {
            for ix in (from + CONTENT_CONTEXT)..(to - CONTENT_CONTEXT) {
                elided.insert(ix);
            }
        }
    }
    elided
}


fn untracked_rows(snap: &DiffSnapshot) -> Vec<DiffRow> {
    let total = snap.untracked_count();
    let shown = &snap.untracked[..snap.untracked.len().min(MAX_RENDERED_FILES)];
    let mut rows = vec![DiffRow::UntrackedHeader { total }];
    for (index, path) in shown.iter().enumerate() {
        rows.push(DiffRow::Untracked {
            index,
            path: path.clone(),
        });
    }
    if total > shown.len() {
        rows.push(DiffRow::MoreUntracked {
            rest: total - shown.len(),
        });
    }
    rows
}

/// Whether a file's lines show on their own, absent an explicit choice.
pub(crate) fn file_expanded(
    file: &FileDiff,
    expanded: &HashMap<String, bool>,
    collapse_all: bool,
) -> bool {
    if let Some(&want) = expanded.get(&file.path) {
        return want;
    }
    !collapse_all && file.added + file.removed <= AUTO_COLLAPSE_LINES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::scm::diff::{DiffLine, Hunk, LineKind};

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: "x".to_string(),
        }
    }

    /// One removal answered by one addition, wrapped in a context line either
    /// side — four lines unified, three rows split.
    fn hunk() -> Hunk {
        Hunk {
            header: "@@ -1,3 +1,3 @@".to_string(),
            lines: vec![
                line(LineKind::Context, Some(1), Some(1)),
                line(LineKind::Removed, Some(2), None),
                line(LineKind::Added, None, Some(2)),
                line(LineKind::Context, Some(3), Some(3)),
            ],
        }
    }

    fn file(path: &str, hunks: usize) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            added: 1,
            removed: 1,
            binary: false,
            truncated: None,
            hunks: std::iter::repeat_n(hunk(), hunks).collect(),
        }
    }

    fn snapshot(files: Vec<FileDiff>) -> DiffSnapshot {
        DiffSnapshot {
            files,
            ..Default::default()
        }
    }

    /// `(path, hunk, row)` for each line row, in list order.
    fn line_coords(rows: &[DiffRow]) -> Vec<(String, usize, usize)> {
        rows.iter()
            .filter_map(|row| match row {
                DiffRow::Split { at, .. } | DiffRow::Unified { at, .. } => {
                    Some((at.path.to_string(), at.id.hunk, at.id.row))
                }
                _ => None,
            })
            .collect()
    }

    /// A row says where it is in the *patch*, not where it is in the list.
    /// Collapsing a file splices the rows below it up the list; a range keyed
    /// on list positions would stay where it was and come away naming other
    /// code.
    #[test]
    fn a_line_row_names_its_file_and_its_place_in_the_hunk() {
        let snap = snapshot(vec![file("a.rs", 1), file("b.rs", 1)]);
        let open = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false, &ContentViews::default());
        let coords = line_coords(&open);
        assert_eq!(
            coords,
            vec![
                ("a.rs".to_string(), 0, 0),
                ("a.rs".to_string(), 0, 1),
                ("a.rs".to_string(), 0, 2),
                ("a.rs".to_string(), 0, 3),
                ("b.rs".to_string(), 0, 0),
                ("b.rs".to_string(), 0, 1),
                ("b.rs".to_string(), 0, 2),
                ("b.rs".to_string(), 0, 3),
            ]
        );

        let collapsed = HashMap::from([("a.rs".to_string(), false)]);
        let after = build_rows(&snap, &collapsed, None, DiffViewMode::Unified, false, &ContentViews::default());
        assert_eq!(
            line_coords(&after),
            coords[4..].to_vec(),
            "b.rs's lines moved up the list and kept the coordinates they had"
        );
    }

    /// Every hunk restarts the row count, so a range that spans two of them is
    /// read hunk by hunk rather than as one run.
    #[test]
    fn each_hunk_numbers_its_own_rows() {
        let snap = snapshot(vec![file("a.rs", 2)]);
        let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Split, false, &ContentViews::default());
        let hunks: Vec<_> = line_coords(&rows)
            .into_iter()
            .map(|(_, hunk, row)| (hunk, row))
            .collect();
        assert_eq!(hunks, vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]);
    }

    fn shape(rows: &[DiffRow]) -> Vec<&'static str> {
        rows.iter()
            .map(|row| match row {
                DiffRow::Gap => "gap",
                DiffRow::Oversized => "oversized",
                DiffRow::FileHeader(_) => "file",
                DiffRow::HunkHeader { .. } => "hunk",
                DiffRow::Split { .. } => "split",
                DiffRow::Unified { .. } => "unified",
                DiffRow::Truncated(_) => "truncated",
                DiffRow::MoreFiles { .. } => "more-files",
                DiffRow::UntrackedHeader { .. } => "untracked-header",
                DiffRow::Untracked { .. } => "untracked",
                DiffRow::MoreUntracked { .. } => "more-untracked",
                DiffRow::ContentBlock(_) => "content-block",
                DiffRow::ContentAnchor(_) => "content-anchor",
                DiffRow::ContentElided { .. } => "content-elided",
                DiffRow::ContentSplit { .. } => "content-split",
                DiffRow::ContentUnified { .. } => "content-unified",
                DiffRow::ContentStatus(_) => "content-status",
            })
            .collect()
    }

    fn open(paths: [&str; 1]) -> HashMap<String, bool> {
        paths.into_iter().map(|p| (p.to_string(), true)).collect()
    }

    #[test]
    fn an_open_file_flattens_to_a_header_a_hunk_header_and_a_row_per_line() {
        let snap = snapshot(vec![file("a.rs", 1)]);
        let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false, &ContentViews::default());
        assert_eq!(
            shape(&rows),
            ["file", "hunk", "unified", "unified", "unified", "unified"]
        );

        let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Split, false, &ContentViews::default());
        assert_eq!(
            shape(&rows),
            ["file", "hunk", "split", "split", "split"],
            "the split view pairs the removal with the addition that replaced it"
        );
    }

    /// A new or deleted file has nothing to set beside it, so split draws it
    /// unified; a modified file in the same list still splits.
    #[test]
    fn split_draws_a_new_or_deleted_file_unified() {
        let added = FileDiff {
            status: FileStatus::Added,
            ..file("new.rs", 1)
        };
        let deleted = FileDiff {
            status: FileStatus::Deleted,
            ..file("gone.rs", 1)
        };
        for one_sided in [added, deleted] {
            let snap = snapshot(vec![one_sided, file("a.rs", 1)]);
            let rows = build_rows(&snap, &open(["new.rs"]), None, DiffViewMode::Split, false, &ContentViews::default());
            let kinds: Vec<_> = shape(&rows)
                .into_iter()
                .filter(|k| matches!(*k, "split" | "unified"))
                .collect();
            assert_eq!(
                kinds,
                [
                    "unified", "unified", "unified", "unified", "split", "split", "split"
                ]
            );
        }
    }

    #[test]
    fn only_the_first_hunk_of_a_file_follows_its_header() {
        let rows = build_rows(
            &snapshot(vec![file("a.rs", 2)]),
            &HashMap::new(),
            None,
            DiffViewMode::Unified,
            false,
            &ContentViews::default(),
        );
        let leads: Vec<bool> = rows
            .iter()
            .filter_map(|r| match r {
                DiffRow::HunkHeader { leads, .. } => Some(*leads),
                _ => None,
            })
            .collect();
        assert_eq!(
            leads,
            [true, false],
            "the second hunk draws the rule that separates it from the first"
        );
    }

    #[test]
    fn a_collapsed_file_is_a_single_row() {
        let snap = snapshot(vec![file("a.rs", 1)]);
        let shut: HashMap<String, bool> = [("a.rs".to_string(), false)].into_iter().collect();
        let rows = build_rows(&snap, &shut, None, DiffViewMode::Unified, false, &ContentViews::default());
        assert_eq!(shape(&rows), ["file"]);
    }

    #[test]
    fn a_truncation_note_lands_under_the_lines_it_is_about() {
        let mut f = file("a.rs", 1);
        f.truncated = Some(Truncation::Budget);
        let rows = build_rows(
            &snapshot(vec![f]),
            &HashMap::new(),
            None,
            DiffViewMode::Unified,
            false,
            &ContentViews::default(),
        );
        assert_eq!(*shape(&rows).last().unwrap(), "truncated");
    }

    #[test]
    fn files_are_separated_by_a_gap_and_the_list_never_opens_with_one() {
        let snap = snapshot(vec![file("a.rs", 1), file("b.rs", 1)]);
        let shut: HashMap<String, bool> =
            snap.files.iter().map(|f| (f.path.clone(), false)).collect();
        let rows = build_rows(&snap, &shut, None, DiffViewMode::Unified, false, &ContentViews::default());
        assert_eq!(shape(&rows), ["file", "gap", "file"]);
    }

    #[test]
    fn the_oversized_banner_leads_and_collapses_what_was_not_asked_for() {
        let snap = snapshot(vec![file("a.rs", 1), file("b.rs", 1)]);
        let rows = build_rows(&snap, &open(["b.rs"]), None, DiffViewMode::Unified, true, &ContentViews::default());
        assert_eq!(
            shape(&rows),
            [
                "oversized",
                "gap",
                "file",
                "gap",
                "file",
                "hunk",
                "unified",
                "unified",
                "unified",
                "unified"
            ],
            "the file the reader opened stays open; the other one does not"
        );
    }

    #[test]
    fn a_focused_file_is_the_only_one_flattened() {
        let snap = snapshot(vec![file("a.rs", 1), file("b.rs", 1)]);
        let rows = build_rows(
            &snap,
            &HashMap::new(),
            Some(1),
            DiffViewMode::Unified,
            false,
            &ContentViews::default(),
        );
        assert_eq!(
            shape(&rows),
            ["file", "hunk", "unified", "unified", "unified", "unified"]
        );
        let DiffRow::FileHeader(head) = &rows[0] else {
            panic!("the focused file leads");
        };
        assert_eq!(head.path, "b.rs");
        assert_eq!(head.index, 1, "the element id still points into `files`");
    }

    #[test]
    fn the_file_list_is_capped_and_the_tail_gets_one_line() {
        let snap = snapshot(
            (0..MAX_RENDERED_FILES + 25)
                .map(|i| {
                    let mut f = file(&format!("f{i}.rs"), 1);
                    f.hunks.clear();
                    f
                })
                .collect(),
        );
        let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false, &ContentViews::default());
        let files = rows
            .iter()
            .filter(|r| matches!(r, DiffRow::FileHeader(_)))
            .count();
        assert_eq!(files, MAX_RENDERED_FILES);
        assert!(matches!(rows.last(), Some(DiffRow::MoreFiles { rest: 25 })));
    }

    #[test]
    fn the_untracked_list_is_capped_but_its_count_stays_true() {
        let mut snap = snapshot(Vec::new());
        snap.untracked = (0..MAX_RENDERED_FILES + 5)
            .map(|i| format!("p{i}"))
            .collect();
        snap.untracked_total = 9_000;
        let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false, &ContentViews::default());

        assert!(matches!(
            rows.first(),
            Some(DiffRow::UntrackedHeader { total: 9_000 })
        ));
        let shown = rows
            .iter()
            .filter(|r| matches!(r, DiffRow::Untracked { .. }))
            .count();
        assert_eq!(shown, MAX_RENDERED_FILES);
        assert!(matches!(
            rows.last(),
            Some(DiffRow::MoreUntracked { rest }) if *rest == 9_000 - MAX_RENDERED_FILES
        ));
    }

    #[test]
    fn a_preview_is_one_open_file() {
        let rows = preview_rows(&file("new.md", 1), DiffViewMode::Unified);
        assert_eq!(
            shape(&rows),
            ["file", "hunk", "unified", "unified", "unified", "unified"]
        );
        let DiffRow::FileHeader(head) = &rows[0] else {
            panic!("a file opens with its header");
        };
        assert_eq!(
            head.index,
            usize::MAX,
            "its element ids stay clear of the real list's"
        );
        assert!(head.expanded);
    }

    #[test]
    fn a_rename_shows_both_names_and_a_binary_file_cannot_be_opened() {
        let mut renamed = file("new.rs", 1);
        renamed.old_path = Some("old.rs".to_string());
        let rows = preview_rows(&renamed, DiffViewMode::Unified);
        let DiffRow::FileHeader(head) = &rows[0] else {
            unreachable!()
        };
        assert_eq!(head.shown_path, "old.rs → new.rs");
        assert_eq!(head.path, "new.rs", "the key is still the file itself");

        let mut binary = file("logo.png", 0);
        binary.binary = true;
        let rows = preview_rows(&binary, DiffViewMode::Unified);
        let DiffRow::FileHeader(head) = &rows[0] else {
            unreachable!()
        };
        assert!(!head.expandable);
    }

    /// The threshold that decides whether a file opens on its own, kept where
    /// the flattening that reads it lives.
    #[test]
    fn a_file_over_the_line_count_opens_only_when_asked() {
        let mut big = file("big.rs", 1);
        big.added = AUTO_COLLAPSE_LINES + 1;
        let none = HashMap::new();
        assert!(!file_expanded(&big, &none, false));
        assert!(file_expanded(&big, &open(["big.rs"]), true));
        assert!(file_expanded(&file("small.rs", 1), &none, false));
    }
}
