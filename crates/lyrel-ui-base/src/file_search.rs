//! The shared file index: what a project's walk found, how a query ranks
//! against it, and the roots a search walks. Owned here rather than inside
//! the Search Everywhere tab because the terminal's command composer reuses
//! the same walk and ranking for its `@` file menu — and the file tree builds
//! its flat listing from the same pieces.
//!
//! Everything here is a value or a `Host` query: no search view, no app
//! state. The tab-specific row building and the app's quick-open orchestration
//! stay in `crate::ui::search::files`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::host_ops::Host;
use crate::fuzzy::fuzzy_score;

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
    /// `rel` lowercased once, for the prefilter that runs on every keystroke.
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
            let lower = rel.to_lowercase();
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

/// Whether every character of `needle` (already lowercase, no whitespace)
/// appears in `hay` in order. Cheap enough to run over the whole index on
/// every keystroke, and it throws out nearly everything before the real
/// scorer, which allocates, sees it.
fn is_subsequence(needle: &[char], hay: &str) -> bool {
    let mut want = needle.iter().peekable();
    for c in hay.chars() {
        match want.peek() {
            Some(w) if **w == c => {
                want.next();
            }
            Some(_) => {}
            None => break,
        }
    }
    want.peek().is_none()
}

/// How well `file` answers `needle`, on the one scale every tab shares — so
/// the All tab can put a file beside an action and have the order mean
/// something. A hit on the file's own name wins over the same hit spread
/// across its directories.
pub fn file_score(needle: &str, file: &IndexedFile) -> Option<i32> {
    let on_name = fuzzy_score(needle, file.name());
    let on_path = fuzzy_score(needle, &file.rel).map(|s| s - PATH_ONLY_PENALTY);
    on_name.into_iter().chain(on_path).max()
}

/// The files that match `needle`, best first, at most `limit` of them. Ties
/// go to the shorter path — the shallower file is the likelier one.
pub fn rank<'a>(index: &'a FileIndex, needle: &str, limit: usize) -> Vec<(i32, &'a IndexedFile)> {
    let chars: Vec<char> = needle
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(i32, &IndexedFile)> = index
        .files
        .iter()
        .filter(|f| is_subsequence(&chars, &f.lower))
        .filter_map(|f| Some((file_score(needle, f)?, f)))
        .collect();
    hits.sort_by(|(a, fa), (b, fb)| b.cmp(a).then_with(|| fa.rel.len().cmp(&fb.rel.len())));
    hits.truncate(limit);
    hits
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
