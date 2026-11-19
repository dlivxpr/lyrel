//! The Files tab: open a file in the editor by typing part of its name, the
//! way ⌘P does in VS Code, instead of walking the tree to it.
//!
//! The list comes from one walk of the project behind the active tab's panes,
//! done on the host that owns them through [`Host::search`]
//! — the same breadth-first, `.gitignore`-aware walk the Files panel's filter
//! runs, asked for every name at once. It is kept between openings and shown
//! at once when the search opens again, while a fresh walk runs behind it
//! (stale-while-revalidate): the list you already had is a better answer
//! than a spinner, and the one that replaces it lands under the cursor
//! without moving the highlighted row.

use std::path::{Path, PathBuf};

use gpui_kit::{App, Context, Window};

use super::SearchTab;
use super::command::{CommandKind, Item};
use super::sources::{Row, Section, Source};
use crate::ui::app::LyrelApp;
use lyrel_ui_base::file_search::{FileList, IndexKey, IndexedFile, parse_query, rank, walk};
use lyrel_ui_base::host_ops::HostOps;
use lyrel_ui_base::i18n::{L10nKey, t_fmt};

#[cfg(test)]
use crate::ui::file_tree::read_probe::{InstanceTicket, ReadKind};

/// Rows the Files tab shows before anything is typed. A sample, shallowest
/// first, so the tab is visibly populated; the query is what finds things.
const BROWSE_ROWS: usize = 100;

/// Rows a query may bring back. Past this the ranking is noise, and every row
/// is an `Item` built on each keystroke.
const SEARCH_ROWS: usize = 200;

/// One row of the Files tab, built from the shared index the terminal's
/// composer also ranks against — the values themselves live in
/// `lyrel_ui_base::file_search`.
fn file_item(file: &IndexedFile, line: Option<u32>, column: Option<u32>) -> Item {
    let mut item = Item::new(
        file.name(),
        CommandKind::OpenFile {
            path: file.path.clone(),
            line,
            column,
        },
    );
    if !file.dir().is_empty() {
        item = item.with_subtitle(file.dir());
    }
    if let Some(line) = line {
        item = item.with_note(t_fmt(
            L10nKey::SearchFilesGoToLine,
            &[("line", &line.to_string())],
        ));
    }
    item
}

pub(super) struct Files<'a>(pub &'a FileList);

impl Source for Files<'_> {
    fn tab(&self) -> SearchTab {
        SearchTab::Files
    }

    fn browse(&self, _cx: &App) -> Vec<Section> {
        let FileList::Ready(index) = self.0 else {
            return Vec::new();
        };
        let rows: Vec<Row> = index
            .files
            .iter()
            .take(BROWSE_ROWS)
            .map(|f| Row::Item(file_item(f, None, None)))
            .collect();
        if rows.is_empty() {
            return Vec::new();
        }
        // A walk that hit its cap left the deepest files out, and a search
        // that cannot find one of them should not look like a missing file.
        let title = index.capped.then(|| {
            t_fmt(
                L10nKey::SearchFilesCapped,
                &[("count", &index.files.len().to_string())],
            )
            .into()
        });
        vec![Section { title, rows }]
    }

    fn highlights(&self, _cx: &App) -> Vec<Item> {
        Vec::new()
    }

    /// A sample of the project is not an answer to anything, and on the All
    /// tab it would push the tabs you were just in down the page.
    fn on_the_empty_all_tab(&self) -> bool {
        false
    }

    fn search(&self, query: &str, _cx: &App) -> Vec<(i32, Item)> {
        let FileList::Ready(index) = self.0 else {
            return Vec::new();
        };
        let q = parse_query(query);
        rank(index, q.needle, SEARCH_ROWS)
            .into_iter()
            .map(|(score, f)| (score, file_item(f, q.line, q.column)))
            .collect()
    }
}

impl LyrelApp {
    /// Search Everywhere on its Files tab — or closed, when that is what it
    /// is already showing, so the chord that opens it also puts it away.
    pub(crate) fn quick_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.view() {
            if view.read(cx).tab() == SearchTab::Files {
                self.close_search(window, cx);
            } else {
                view.update(cx, |view, cx| {
                    view.set_tab(SearchTab::Files, None, window, cx)
                });
            }
            return;
        }
        self.open_search(SearchTab::Files, "", window, cx);
    }

    /// Opens a row of the Files tab, and has the tree follow along the way a
    /// file link in the grid does.
    pub(crate) fn open_indexed_file(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_in_editor_at(path, line, column, window, cx);
        self.file_tree_reveal_path(path, cx);
    }

    fn file_index_key(&self, cx: &App) -> IndexKey {
        let host = self.spawn_host(cx);
        let cwds = self.file_project_cwds(cx);
        IndexKey::new(host, cwds)
    }

    /// What the Files tab opens on: the last walk, if it was of the folders
    /// the panes are in now.
    pub(crate) fn file_list_now(&self, cx: &App) -> FileList {
        let key = self.file_index_key(cx);
        if key.cwds().is_empty() {
            return FileList::NoRoots;
        }
        if !self
            .active_host(cx)
            .is_some_and(|host| self.file_tree.read(cx).host_matches(&host))
        {
            return FileList::Indexing;
        }
        self.file_tree.read(cx).file_list(&key)
    }

    /// In-place entry and reconnect also replace retired catalog rows when
    /// the current source needs no walk, including a workspace with no roots.
    pub(crate) fn refresh_search_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_file_index(window, cx);
        if let Some(view) = self.search.view() {
            let files = self.file_list_now(cx);
            view.update(cx, |view, cx| view.set_files(files, window, cx));
        }
    }

    /// Walks the project again, unless a walk of it just landed or is
    /// already on its way, and hands the result to the open search.
    pub(crate) fn refresh_file_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.file_index_key(cx);
        let Some(host) = self.active_host(cx) else {
            return;
        };
        self.file_tree_sync_host(&host, cx);
        let Some(generation) = self
            .file_tree
            .update(cx, |tree, _| tree.begin_file_index(&key))
        else {
            return;
        };
        // Only this computer's home is known here, and it is the common case:
        // a fresh local tab starts there.
        let home = host
            .id()
            .is_local()
            .then(|| std::env::var_os("HOME").map(PathBuf::from))
            .flatten();
        let cwds = key.cwds().to_vec();
        #[cfg(test)]
        let landing_host = InstanceTicket::of(&host);
        let authority = self.file_tree.read(cx).read_authority(&host);
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| walk(h, &cwds, home.as_deref()),
            move |app, list, window, cx| {
                #[cfg(test)]
                app.file_tree
                    .update(cx, |tree, _| {
                        tree.read_probe.record(ReadKind::Index, landing_host);
                    });
                let tree = app.file_tree.read(cx);
                if !authority.is_current(tree, cx) {
                    return;
                }
                if !app
                    .file_tree
                    .update(cx, |tree, _| tree.finish_file_index(generation, key.clone(), list))
                {
                    return;
                }
                if app.file_index_key(cx) != key {
                    return;
                }
                let list = app.file_list_now(cx);
                if let Some(view) = app.search.view() {
                    view.update(cx, |view, cx| view.set_files(list, window, cx));
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use lyrel_ui_base::file_search::{
        FileIndex, FileQuery, INDEX_LIMIT, build_index, searchable_roots,
    };

    fn index(paths: &[&str]) -> FileIndex {
        build_index(
            &[PathBuf::from("/repo")],
            paths
                .iter()
                .map(|p| PathBuf::from(format!("/repo/{p}")))
                .collect(),
            false,
        )
    }

    fn ranked(index: &FileIndex, query: &str) -> Vec<String> {
        rank(index, parse_query(query).needle, 50)
            .into_iter()
            .map(|(_, f)| f.rel.clone())
            .collect()
    }

    #[test]
    fn a_query_can_name_a_line_and_a_column() {
        let q = |s| parse_query(s);
        assert_eq!(
            q("main.rs"),
            FileQuery {
                needle: "main.rs",
                line: None,
                column: None
            }
        );
        assert_eq!(
            q("main.rs:120"),
            FileQuery {
                needle: "main.rs",
                line: Some(120),
                column: None
            }
        );
        assert_eq!(
            q("src/main.rs:120:7"),
            FileQuery {
                needle: "src/main.rs",
                line: Some(120),
                column: Some(7)
            }
        );
        // Half typed: the colon is on its way to a number.
        assert_eq!(q("main.rs:").needle, "main.rs");
        assert_eq!(q("main.rs:12:").line, Some(12));
        // Not a number, so part of the name.
        assert_eq!(q("std::fs").needle, "std::fs");
        assert_eq!(q("a:b12").needle, "a:b12");
        // A bare number is a name to look for, not a line in nothing.
        assert_eq!(q("404").needle, "404");
        assert_eq!(q(":12").needle, ":12");
        // Line 0 does not exist.
        assert_eq!(q("x.rs:0").line, None);
        assert_eq!(q("  x.rs:3  ").line, Some(3));
    }

    #[test]
    fn a_match_on_the_name_beats_the_same_letters_in_the_path() {
        let idx = index(&["apps/web/index.ts", "src/app.rs", "src/ui/app.rs"]);
        let got = ranked(&idx, "app");
        assert_eq!(
            &got[..2],
            ["src/app.rs", "src/ui/app.rs"],
            "the shorter of two equal name hits leads"
        );
        assert_eq!(got[2], "apps/web/index.ts");
    }

    #[test]
    fn an_exact_name_outranks_a_longer_one() {
        let idx = index(&["src/ui/main_window.rs", "src/main.rs"]);
        assert_eq!(ranked(&idx, "main.rs")[0], "src/main.rs");
    }

    #[test]
    fn a_path_query_finds_files_by_their_directories() {
        let idx = index(&["src/ui/search/view.rs", "src/ui/view.rs", "docs/view.md"]);
        let got = ranked(&idx, "search/view");
        assert_eq!(got, ["src/ui/search/view.rs"]);
        let got = ranked(&idx, "ui view");
        assert_eq!(got.len(), 2, "whitespace is ignored, as everywhere else");
    }

    #[test]
    fn nothing_that_is_not_a_subsequence_matches() {
        let idx = index(&["src/main.rs"]);
        assert!(ranked(&idx, "zzz").is_empty());
        assert!(ranked(&idx, "").is_empty(), "an empty query ranks nothing");
    }

    #[test]
    fn rows_split_into_a_name_and_the_directory_it_is_in() {
        let idx = index(&["src/ui/app.rs", "README.md"]);
        assert_eq!(idx.files[0].name(), "app.rs");
        assert_eq!(idx.files[0].dir(), "src/ui");
        assert_eq!(idx.files[1].name(), "README.md");
        assert_eq!(idx.files[1].dir(), "");
    }

    #[test]
    fn two_roots_are_told_apart_by_name() {
        let idx = build_index(
            &[PathBuf::from("/work/api"), PathBuf::from("/work/web")],
            vec![
                PathBuf::from("/work/api/src/main.rs"),
                PathBuf::from("/work/web/src/main.rs"),
                // A sibling whose name merely starts with a root's is not in it.
                PathBuf::from("/work/api-old/x.rs"),
            ],
            false,
        );
        let rels: Vec<_> = idx.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["api/src/main.rs", "web/src/main.rs"]);
    }

    #[test]
    fn a_windows_path_splits_on_its_own_separator() {
        let idx = build_index(
            &[PathBuf::from(r"C:\repo")],
            vec![PathBuf::from(r"C:\repo\src\lib.rs")],
            false,
        );
        assert_eq!(idx.files[0].rel, r"src\lib.rs");
        assert_eq!(idx.files[0].name(), "lib.rs");
        assert_eq!(idx.files[0].dir(), "src");
    }

    #[test]
    fn home_and_the_filesystem_root_are_not_projects() {
        let home = PathBuf::from("/home/me");
        let roots = searchable_roots(
            vec![
                home.clone(),
                PathBuf::from("/"),
                PathBuf::from("/home/me/repo"),
                PathBuf::from("/home/me/repo"),
                PathBuf::from("/home/me/repo/crates/core"),
                PathBuf::from("/srv/other"),
            ],
            Some(&home),
        );
        assert_eq!(
            roots,
            [PathBuf::from("/home/me/repo"), PathBuf::from("/srv/other")],
            "home and / go, a duplicate goes, and a folder inside another is covered by it"
        );
        assert!(searchable_roots(vec![home.clone()], Some(&home)).is_empty());
    }

    /// The walk goes through the host, from the pane's directory up to its
    /// repository, and leaves out what the tree hides by default.
    #[test]
    fn the_walk_indexes_the_repository_without_what_git_ignores() {
        let host = lyrel_core::host::local::LocalHost::new();
        let tmp = std::env::temp_dir().join(format!("lyrel-quick-open-{}", std::process::id()));
        let _ = host.remove(&tmp, true);
        host.create_dir(&tmp.join(".git"), true).unwrap();
        host.create_dir(&tmp.join("src/ui"), true).unwrap();
        host.create_dir(&tmp.join("target/debug"), true).unwrap();
        host.write_file(&tmp.join(".gitignore"), b"target/\n")
            .unwrap();
        host.write_file(&tmp.join(".git/HEAD"), b"").unwrap();
        host.write_file(&tmp.join("target/debug/out.rs"), b"")
            .unwrap();
        host.write_file(&tmp.join("README.md"), b"").unwrap();
        host.write_file(&tmp.join("src/ui/app.rs"), b"").unwrap();

        // From a subdirectory: the walk is of the whole repository.
        let FileList::Ready(idx) = walk(&*host, &[tmp.join("src")], None) else {
            panic!("the walk failed");
        };
        let mut rels: Vec<_> = idx.files.iter().map(|f| f.rel.replace('\\', "/")).collect();
        rels.sort();
        assert_eq!(rels, ["README.md", "src/ui/app.rs"]);
        assert!(!idx.capped);

        assert!(matches!(
            walk(&*host, std::slice::from_ref(&tmp), Some(&tmp)),
            FileList::NoRoots
        ));
        let _ = host.remove(&tmp, true);
    }

    #[test]
    fn a_long_index_is_ranked_quickly_enough_to_type_into() {
        let paths: Vec<String> = (0..INDEX_LIMIT)
            .map(|i| format!("crates/c{}/src/module_{i}/file_{i}.rs", i % 97))
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let idx = index(&refs);
        let started = Instant::now();
        let got = rank(&idx, "file_4242", 20);
        assert_eq!(got[0].1.name(), "file_4242.rs");
        // Generous, so a loaded CI box does not flake; the point is that a
        // full index is not seconds of work per keystroke.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "ranking {INDEX_LIMIT} files took {:?}",
            started.elapsed()
        );
    }
}
