//! The shared file index: what a project's walk found, how a query ranks
//! against it, and the roots a search walks. Owned here rather than inside
//! the Search Everywhere tab because the terminal's command composer reuses
//! the same walk and ranking for its `@` file menu — and the file tree builds
//! its flat listing from the same pieces.
//!
//! Everything here is a value or a `Host` query: no search view, no app
//! state. The tab-specific row building and the app's quick-open orchestration
//! stay in `crate::ui::search::files`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fuzzy::score_folded;
use crate::host_ops::Host;

/// How many entries one walk may return, directories included. A project
/// past this is mostly generated or vendored files, and a list the size of
/// the cap is already more than anyone scrolls; the walk is breadth-first, so
/// what the cap cuts off is the deepest part of the tree.
pub const INDEX_LIMIT: usize = 50_000;

/// How many directories one walk may open. Bounds a walk over a slow link as
/// much as it bounds the list: each directory is a round trip on a remote host.
pub const INDEX_MAX_DIRS: usize = 20_000;

/// What a match only on the directory part of a path gives up against one on
/// the file's own name. Typing `app` means `app.rs` far more often than
/// `apps/web/index.ts`, but a query with a `/` in it can only match the path,
/// and those still have to come back.
const PATH_ONLY_PENALTY: i32 = 15;

/// One file the walk found.
#[derive(Debug)]
pub struct IndexedFile {
    /// Where it is on the host, as the host spells it.
    pub path: PathBuf,
    /// Where it is under its project root — the text the query matches and
    /// the row shows. With more than one root it starts with the root's name,
    /// so two `src/main.rs` in two projects can be told apart.
    pub rel: String,
    /// Where the file's own name starts in `rel`.
    pub name_at: usize,
    /// `rel` case-folded once, for the prefilter and scoring.
    lower: String,
}

impl IndexedFile {
    pub fn name(&self) -> &str {
        &self.rel[self.name_at..]
    }

    /// The directory part of `rel`, without its trailing separator. Empty for
    /// a file at the root.
    pub fn dir(&self) -> &str {
        self.rel[..self.name_at].trim_end_matches(['/', '\\'])
    }
}

#[derive(Debug, Default)]
pub struct FileIndex {
    /// In the order the walk found them: shallowest first.
    pub files: Vec<IndexedFile>,
    /// Whether the walk stopped at [`INDEX_LIMIT`]. One cut short by
    /// [`INDEX_MAX_DIRS`] cannot tell, and is not flagged.
    pub capped: bool,
}

/// What the Files tab has to offer right now.
#[derive(Clone, Debug, Default)]
pub enum FileList {
    /// No pane has a directory worth searching: none reported one, or the one
    /// it reported is the home directory, which is not a project and whose
    /// walk would be all caches and downloads.
    #[default]
    NoRoots,
    /// The first walk of these folders has not come back yet.
    Indexing,
    /// The host could not walk them.
    Failed,
    Ready(Arc<FileIndex>),
}

/// A query as the Files tab reads it: the name to look for, and the place in
/// the file to land on when `:line` or `:line:column` follows it — what a
/// compiler prints and what people paste.
#[derive(Debug, PartialEq, Eq)]
pub struct FileQuery<'a> {
    pub needle: &'a str,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

pub fn parse_query(query: &str) -> FileQuery<'_> {
    let query = query.trim();
    // A trailing colon is a line number on its way; it is not part of a name.
    let mut rest = query.strip_suffix(':').unwrap_or(query);
    let mut numbers: Vec<u32> = Vec::new();
    while numbers.len() < 2 {
        let Some((head, tail)) = rest.rsplit_once(':') else {
            break;
        };
        if head.is_empty() || tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        let Ok(n) = tail.parse::<u32>() else { break };
        numbers.push(n);
        rest = head;
    }
    // Read right to left, so the last number found is the line.
    numbers.reverse();
    let nonzero = |n: Option<&u32>| n.copied().filter(|n| *n > 0);
    FileQuery {
        needle: rest,
        line: nonzero(numbers.first()),
        column: nonzero(numbers.get(1)),
    }
}

/// The index for `files` found under `roots`.
pub fn build_index(roots: &[PathBuf], files: Vec<PathBuf>, capped: bool) -> FileIndex {
    let many = roots.len() > 1;
    let files = files
        .into_iter()
        .filter_map(|path| {
            let text = path.to_string_lossy().into_owned();
            // By text rather than `Path::strip_prefix`: a remote host's paths
            // may be Windows paths this client parses as one component.
            let (root, under) = roots
                .iter()
                .filter_map(|root| {
                    let root = root.to_string_lossy();
                    let under = text.strip_prefix(root.as_ref())?;
                    let under = under.trim_start_matches(['/', '\\']);
                    // `/repo-old/x` is not under `/repo`.
                    (under.len() < text.len() - root.len() || root.ends_with(['/', '\\']))
                        .then(|| (root.into_owned(), under.to_string()))
                })
                .max_by_key(|(root, _)| root.len())?;
            if under.is_empty() {
                return None;
            }
            let rel = match many {
                true => {
                    let name = root
                        .trim_end_matches(['/', '\\'])
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or_default();
                    format!("{name}/{under}")
                }
                false => under,
            };
            let name_at = rel.rfind(['/', '\\']).map_or(0, |i| i + 1);
            let lower = if rel.is_ascii() {
                rel.to_ascii_lowercase()
            } else {
                fold(&rel).collect()
            };
            Some(IndexedFile {
                path,
                rel,
                name_at,
                lower,
            })
        })
        .collect();
    FileIndex { files, capped }
}

fn fold(text: &str) -> impl Iterator<Item = char> + '_ {
    // Lowercase each scalar independently and use the same form for both
    // lowercase sigmas, regardless of their position in the path or query.
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ς' { 'σ' } else { c })
}

/// Whether every character of `needle` (already lowercase, no whitespace)
/// appears in `hay` in order. Reject misses before scoring either substring.
fn is_subsequence(needle: &[char], hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.iter().all(|&want| hay.any(|c| c == want))
}

/// How well `file` answers `needle`, on the one scale every tab shares — so
/// the All tab can put a file beside an action and have the order mean
/// something. A hit on the file's own name wins over the same hit spread
/// across its directories.
fn file_score(needle: &[char], file: &IndexedFile, scratch: &mut Vec<char>) -> Option<i32> {
    fn score(needle: &[char], hay: &[impl Copy + Into<char>], name_at: usize) -> Option<i32> {
        let on_name = score_folded(needle, &hay[name_at..]);
        if name_at == 0 {
            return on_name;
        }
        let on_path = score_folded(needle, hay).map(|s| s - PATH_ONLY_PENALTY);
        on_name.max(on_path)
    }

    if file.rel.is_ascii() {
        return score(needle, file.lower.as_bytes(), file.name_at);
    }

    // Separators are unchanged by folding, but the basename can start at a
    // different byte or scalar offset when a lowercase mapping expands.
    let name_at = file.lower.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let (dir, name) = file.lower.split_at(name_at);
    scratch.clear();
    scratch.extend(dir.chars());
    let name_at = scratch.len();
    scratch.extend(name.chars());
    score(needle, scratch, name_at)
}

/// The files that match `needle`, best first, at most `limit` of them. Ties
/// go to the shorter path, then keep the index's order.
pub fn rank<'a>(index: &'a FileIndex, needle: &str, limit: usize) -> Vec<(i32, &'a IndexedFile)> {
    if limit == 0 {
        return Vec::new();
    }
    let needle: Vec<_> = fold(needle).filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() {
        return Vec::new();
    }

    let mut scratch = Vec::new();
    // The worst retained match sorts last and sits at the top of the heap.
    let mut hits = BinaryHeap::new();
    for (order, file) in index.files.iter().enumerate() {
        if !is_subsequence(&needle, &file.lower) {
            continue;
        }
        let Some(score) = file_score(&needle, file, &mut scratch) else {
            continue;
        };
        let hit = (Reverse(score), file.rel.len(), order);
        if hits.len() < limit {
            hits.push(hit);
        } else if let Some(mut worst) = hits.peek_mut()
            && hit < *worst
        {
            *worst = hit;
        }
    }
    hits.into_sorted_vec()
        .into_iter()
        .map(|(Reverse(score), _, order)| (score, &index.files[order]))
        .collect()
}

/// Walks the project the panes in `cwds` are in, on the host that has it.
/// Runs off the UI thread.
pub fn walk(h: &dyn Host, cwds: &[PathBuf], home: Option<&Path>) -> FileList {
    let resolved = cwds
        .iter()
        .map(|cwd| {
            h.repo_root(cwd)
                .ok()
                .flatten()
                .unwrap_or_else(|| cwd.clone())
        })
        .collect();
    let roots = searchable_roots(resolved, home);
    if roots.is_empty() {
        return FileList::NoRoots;
    }
    // Hidden and gitignored entries stay out, as they do in the tree by
    // default: that is what keeps `.git`, `target/` and `node_modules/` from
    // filling the cap before the sources. The empty query matches every name.
    match h.search(&roots, "", INDEX_LIMIT, INDEX_MAX_DIRS, false) {
        Ok(hits) => {
            let capped = hits.len() >= INDEX_LIMIT;
            let files = hits
                .into_iter()
                .filter(|hit| !hit.is_dir)
                .map(|hit| hit.path)
                .collect();
            FileList::Ready(Arc::new(build_index(&roots, files, capped)))
        }
        Err(e) => {
            log::warn!("quick open: walking {roots:?}: {e}");
            FileList::Failed
        }
    }
}

/// Who a walk or a listing was for: the host the paths live on, and the
/// directories keyed, sorted so two spellings of the same set are one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexKey {
    host: crate::host_ops::HostId,
    cwds: Vec<PathBuf>,
}

impl IndexKey {
    pub fn new(host: crate::host_ops::HostId, mut cwds: Vec<PathBuf>) -> Self {
        cwds.sort();
        cwds.dedup();
        Self { host, cwds }
    }

    pub fn cwds(&self) -> &[PathBuf] {
        &self.cwds
    }
}

/// Quick open excludes HOME, filesystem roots and redundant nested roots.
/// Panel content search instead uses the resolved pane roots without fallback.
pub fn searchable_roots(resolved: Vec<PathBuf>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in resolved {
        if home.is_some_and(|home| root == home) || root.parent().is_none() || roots.contains(&root)
        {
            continue;
        }
        roots.push(root);
    }
    let all = roots.clone();
    roots.retain(|root| {
        !all.iter()
            .any(|other| other != root && root.starts_with(other))
    });
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index<'a>(names: impl IntoIterator<Item = &'a str>) -> FileIndex {
        let root = Path::new("/repo");
        build_index(
            &[root.to_path_buf()],
            names.into_iter().map(|n| root.join(n)).collect(),
            false,
        )
    }

    #[test]
    fn broad_file_queries_keep_ties_in_index_order_at_both_result_limits() {
        let numbered: Vec<_> = (0..240).rev().map(|i| format!("file_{i:05}.txt")).collect();
        let index = index(numbered.iter().map(String::as_str).chain([
            "frost-b.txt",
            "f文-b.txt",
            "flare-b.txt",
            "fritz.txt",
            "fable.txt",
            "f文-a.txt",
            "flare-a.txt",
            "frost-a.txt",
        ]));
        let expected: Vec<_> = [
            "fritz.txt",
            "fable.txt",
            "f文-b.txt",
            "f文-a.txt",
            "frost-b.txt",
            "flare-b.txt",
            "flare-a.txt",
            "frost-a.txt",
        ]
        .into_iter()
        .chain(numbered.iter().map(String::as_str))
        .collect();
        for limit in [0, 8, 200, 300] {
            let paths: Vec<_> = rank(&index, "F", limit)
                .into_iter()
                .map(|(_, file)| file.rel.as_str())
                .collect();
            assert_eq!(paths, expected[..limit.min(expected.len())]);
        }
    }

    #[test]
    fn file_queries_keep_unicode_lowercase_expansion_and_sigma_matching() {
        let index = index(["İ/İ.rs", "İ/src/Note.rs", "ΟΣ", "οσ"]);
        let ranked = |query| {
            rank(&index, query, 8)
                .into_iter()
                .map(|(score, file)| (score, file.rel.as_str()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ranked(" \u{2003}İ . R S\t"),
            [(205, "İ/İ.rs"), (44, "İ/src/Note.rs")]
        );
        for query in ["ΟΣ", "ος", "οσ"] {
            assert_eq!(ranked(query), [(151, "ΟΣ"), (151, "οσ")], "{query}");
        }
    }
}
