//! File-tree transitions. GPUI adapters supply Host/tab context, run I/O and
//! execute navigation; they never manipulate cache or operation bookkeeping.

use super::*;
use crate::ui::search::files::FileList;
use std::time::{Duration, Instant};

const REVALIDATE_AFTER: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexKey {
    host: HostId,
    cwds: Vec<PathBuf>,
}

impl IndexKey {
    pub(crate) fn new(host: HostId, mut cwds: Vec<PathBuf>) -> Self {
        cwds.sort();
        cwds.dedup();
        Self { host, cwds }
    }

    pub(crate) fn cwds(&self) -> &[PathBuf] {
        &self.cwds
    }
}

struct Walked {
    key: IndexKey,
    list: FileList,
    at: Instant,
}

#[derive(Default)]
pub(super) struct FileIndexStore {
    last: Option<Walked>,
    walking: Option<(u64, IndexKey)>,
    generation: u64,
}

pub(super) struct Refresh {
    pub notify: bool,
    pub reload: bool,
}

/// Browsing may fall back to HOME; no search consumer receives this fallback.
pub(super) fn browsable_roots(mut roots: Vec<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    if roots.is_empty() {
        roots.extend(home);
    }
    roots
}

/// Quick open excludes HOME, filesystem roots and redundant nested roots.
/// Panel content search instead uses the resolved pane roots without fallback.
pub(crate) fn searchable_roots(resolved: Vec<PathBuf>, home: Option<&Path>) -> Vec<PathBuf> {
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

impl FileTreeState {
    pub(super) fn resolve_project_roots(
        &mut self,
        host: &SharedHost,
        cwds: &[PathBuf],
        cx: &mut Context<LyrelApp>,
    ) -> Option<Vec<PathBuf>> {
        let mut roots = Vec::new();
        let mut resolved = true;
        for cwd in cwds {
            match self.repo_roots.get(host.id(), cwd) {
                Some(root) => {
                    if !roots.contains(root) {
                        roots.push(root.clone());
                    }
                }
                None => {
                    resolved = false;
                    self.request_repo_root(host, cwd.clone(), cx);
                }
            }
        }
        resolved.then_some(roots)
    }

    fn request_repo_root(&mut self, host: &SharedHost, cwd: PathBuf, cx: &mut Context<LyrelApp>) {
        let key = (host.id(), cwd.clone());
        if !self.repo_root_loads.begin(key.clone()) {
            return;
        }
        HostOps::run(
            host.clone(),
            cx,
            {
                let cwd = cwd.clone();
                move |host| {
                    let root = host.repo_root(&cwd).ok().flatten().unwrap_or(cwd);
                    let real = host.canonicalize(&root).ok().filter(|real| *real != root);
                    (root, real)
                }
            },
            move |app, (root, real), cx| {
                app.file_tree.finish_repo_root(&key, cwd, root, real);
                cx.notify();
            },
        );
    }

    pub(super) fn search_is_current(&self, generation: u64) -> bool {
        self.search.generation == generation
    }

    pub(super) fn finish_search(
        &mut self,
        generation: u64,
        ok: bool,
        hits: Vec<TreeEntry>,
    ) -> bool {
        let accepted = self.search.accept(generation, ok, hits);
        if accepted {
            self.invalidate_rows();
        }
        accepted
    }

    pub(crate) fn cancel_edit(&mut self) {
        self.editing = None;
        self.editing_subs.clear();
    }

    pub(super) fn finish_listing(
        &mut self,
        key: &DirKey,
        dir: PathBuf,
        readable: bool,
        entries: Vec<TreeEntry>,
        active_host: Option<HostId>,
    ) -> Landed {
        let note_changed = if readable {
            self.unreadable.remove(key)
        } else {
            self.unreadable.insert(key.clone())
        };
        if note_changed {
            self.invalidate_rows();
        }
        let mut landed = self.land_load(key, key.0, dir, entries);
        landed.changed |= note_changed;
        // Invalidated work remains visible. Only its replacement is conditional
        // on the current Host; an inactive Host refreshes on its next display.
        landed.superseded = landed.superseded && active_host == Some(key.0);
        if landed.superseded {
            self.loads.begin(key.clone());
        }
        landed
    }

    pub(super) fn finish_watch_update(&mut self, result: std::io::Result<()>) -> bool {
        self.watch_busy = false;
        if let Err(e) = result {
            log::warn!("file tree: could not update the watched set: {e}");
        }
        std::mem::take(&mut self.watch_dirty)
    }

    pub(super) fn finish_watch_open(
        &mut self,
        result: std::io::Result<Arc<WatchSub>>,
        host: SharedHost,
        opened_with: &HashSet<PathBuf>,
    ) -> Option<(smol::channel::Receiver<Vec<PathBuf>>, bool)> {
        if !self
            .watch_host
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &host))
        {
            return None;
        }
        self.watch_opening = false;
        let sub = match result {
            Ok(sub) => sub,
            Err(e) => {
                log::warn!("file tree: watcher unavailable: {e}");
                return None;
            }
        };
        let events = sub.events().clone();
        self.watch = Some(sub);
        self.watch_host = Some(host);
        Some((events, self.watched != *opened_with))
    }

    pub(super) fn apply_fs_events(
        &mut self,
        host: HostId,
        paths: &HashSet<PathBuf>,
        on_screen: bool,
        listings_on_screen: bool,
        active_host: HostId,
    ) -> Refresh {
        let roots_moved = paths.iter().any(|path| {
            path.file_name().is_some_and(|name| name == ".git")
                || path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == ".git")
        }) && self.invalidate_repo_roots();
        if paths
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == ".gitignore"))
            && self.gitignore_reaches_tree(host, paths)
        {
            self.invalidate_all();
            return Refresh {
                notify: on_screen,
                reload: false,
            };
        }
        let mut touched = false;
        for dir in dirs_to_relist(paths, self.show_hidden) {
            touched |= self.invalidate_dir(host, dir);
        }
        // Polling backends can report only the directory itself.
        for dir in paths {
            touched |= self.invalidate_dir(host, dir);
        }
        Refresh {
            notify: roots_moved && on_screen,
            reload: touched && listings_on_screen && active_host == host,
        }
    }

    pub(super) fn finish_repo_root(
        &mut self,
        key: &DirKey,
        cwd: PathBuf,
        root: PathBuf,
        real: Option<PathBuf>,
    ) {
        if self.repo_root_loads.finish(key) {
            if let Some(real) = real {
                self.root_aliases.insert(key.0, root.clone(), real);
            }
            self.repo_roots.insert(key.0, cwd, root);
        }
    }

    pub(crate) fn project_spelling(&self, host: HostId, roots: &[PathBuf], path: &Path) -> PathBuf {
        if roots.iter().any(|root| path.starts_with(root)) {
            return path.to_path_buf();
        }
        roots
            .iter()
            .find_map(|root| {
                let real = self.root_aliases.get(host, root)?;
                Some(root.join(path.strip_prefix(real).ok()?))
            })
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// A failed optimistic operation evicts the guess, never restores an old
    /// snapshot over a listing that may have arrived while the write ran.
    pub(super) fn finish_write<T>(
        &mut self,
        host: HostId,
        dir: &Path,
        result: std::io::Result<T>,
        before: Option<Vec<TreeEntry>>,
    ) -> std::io::Result<T> {
        match result {
            Ok(value) => {
                self.invalidate_dir(host, dir);
                Ok(value)
            }
            Err(error) => {
                self.rollback(host, dir, before);
                Err(error)
            }
        }
    }

    pub(super) fn copied_into(&mut self, host: HostId, dir: &Path, report: &file_copy::DropReport) {
        if !report.copied.is_empty() {
            self.invalidate_dir(host, dir);
        }
    }

    pub(crate) fn file_list(&self, key: &IndexKey) -> FileList {
        if key.cwds.is_empty() {
            return FileList::NoRoots;
        }
        match &self.quick_open.last {
            Some(last) if last.key == *key => last.list.clone(),
            _ => FileList::Indexing,
        }
    }

    pub(crate) fn begin_file_index(&mut self, key: &IndexKey) -> Option<u64> {
        let store = &mut self.quick_open;
        if key.cwds.is_empty()
            || store
                .walking
                .as_ref()
                .is_some_and(|(_, walking)| walking == key)
            || store
                .last
                .as_ref()
                .is_some_and(|last| last.key == *key && last.at.elapsed() < REVALIDATE_AFTER)
        {
            return None;
        }
        store.generation += 1;
        store.walking = Some((store.generation, key.clone()));
        Some(store.generation)
    }

    pub(crate) fn finish_file_index(
        &mut self,
        generation: u64,
        key: IndexKey,
        list: FileList,
    ) -> bool {
        let store = &mut self.quick_open;
        if store.walking.as_ref().map(|(generation, _)| *generation) != Some(generation) {
            return false;
        }
        store.walking = None;
        let keep_old = matches!(list, FileList::Failed)
            && store
                .last
                .as_ref()
                .is_some_and(|last| last.key == key && matches!(last.list, FileList::Ready(_)));
        if !keep_old {
            store.last = Some(Walked {
                key,
                list,
                at: Instant::now(),
            });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window;
    use gpui_kit::TestAppContext;

    fn entry(name: &str) -> TreeEntry {
        TreeEntry {
            name: name.into(),
            path: PathBuf::from("/project").join(name),
            is_dir: false,
            ignored: false,
        }
    }

    #[gpui_kit::test]
    fn invalidated_load_is_visible_and_reloads_only_its_host(cx: &mut TestAppContext) {
        let (app, mut vcx) = test_window::harness(cx);
        app.update_in(&mut vcx, |app, _, _| {
            let tree = &mut app.file_tree;
            let dir = PathBuf::from("/project");
            let local = (HostId::LOCAL, dir.clone());
            let remote = (HostId(99), dir.clone());
            tree.loads.begin(local.clone());
            tree.loads.begin(remote.clone());
            let paths = HashSet::from([dir.join("changed.rs")]);
            assert!(
                tree.apply_fs_events(local.0, &paths, true, true, local.0)
                    .reload
            );
            let landed = tree.finish_listing(
                &local,
                dir.clone(),
                true,
                vec![entry("old.rs")],
                Some(local.0),
            );
            assert!(landed.superseded);
            assert!(tree.loads.is_pending(&local));
            let rows = tree.visible_rows(local.0, std::slice::from_ref(&dir), &HashSet::new());
            assert_eq!(rows[1].entry.name, "old.rs");
            tree.invalidate_dir(remote.0, &dir);
            let landed = tree.finish_listing(
                &remote,
                dir.clone(),
                true,
                vec![entry("remote.rs")],
                Some(local.0),
            );
            assert!(!landed.superseded);
            assert!(!tree.loads.is_pending(&remote));
            tree.finish_listing(
                &local,
                dir.clone(),
                true,
                vec![entry("new.rs")],
                Some(local.0),
            );
            let rows = tree.visible_rows(local.0, std::slice::from_ref(&dir), &HashSet::new());
            assert_eq!(rows[1].entry.name, "new.rs");
            let rows = tree.visible_rows(remote.0, &[dir], &HashSet::new());
            assert_eq!(rows[1].entry.name, "remote.rs");
        });
    }

    #[gpui_kit::test]
    fn write_outcomes_relist_without_restoring_an_obsolete_snapshot(cx: &mut TestAppContext) {
        let (app, mut vcx) = test_window::harness(cx);
        app.update_in(&mut vcx, |app, _, _| {
            let tree = &mut app.file_tree;
            let dir = PathBuf::from("/project");
            let key = (HostId::LOCAL, dir.clone());
            tree.children
                .insert(key.0, dir.clone(), vec![entry("old.rs")]);
            let before = tree.optimistic(key.0, &dir, &TreeWrite::NewFile, &entry("new.rs"));
            tree.children
                .insert(key.0, dir.clone(), vec![entry("external.rs")]);
            let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
            assert!(
                tree.finish_write::<()>(key.0, &dir, Err(denied), before)
                    .is_err()
            );
            let rows = tree.visible_rows(key.0, std::slice::from_ref(&dir), &HashSet::new());
            assert_eq!(rows[1].note, Some(TreeNote::Loading));
            tree.children
                .insert(key.0, dir.clone(), vec![entry("external.rs")]);
            let before = tree.optimistic(key.0, &dir, &TreeWrite::NewFolder, &entry("created"));
            tree.finish_write(key.0, &dir, Ok(()), before).unwrap();
            assert!(tree.stale.contains(&key));
            let rows = tree.visible_rows(key.0, &[dir], &HashSet::new());
            assert!(rows.iter().any(|row| row.entry.name == "created"));
        });
    }

    #[gpui_kit::test]
    fn index_revalidation_keeps_usable_files_and_fences_host_changes(cx: &mut TestAppContext) {
        let (app, mut vcx) = test_window::harness(cx);
        app.update_in(&mut vcx, |app, _, _| {
            let tree = &mut app.file_tree;
            let local = IndexKey::new(HostId::LOCAL, vec![PathBuf::from("/project")]);
            let remote = IndexKey::new(HostId(99), vec![PathBuf::from("/project")]);
            let old = tree.begin_file_index(&local).unwrap();
            assert!(tree.begin_file_index(&local).is_none());
            let current = tree.begin_file_index(&remote).unwrap();
            assert!(!tree.finish_file_index(old, local.clone(), FileList::Failed));
            let index = Arc::new(crate::ui::search::files::build_index(
                &[PathBuf::from("/project")],
                vec![PathBuf::from("/project/main.rs")],
                false,
            ));
            assert!(tree.finish_file_index(
                current,
                remote.clone(),
                FileList::Ready(index.clone())
            ));
            assert!(matches!(tree.file_list(&local), FileList::Indexing));
            tree.quick_open.last.as_mut().unwrap().at = Instant::now() - Duration::from_secs(6);
            let refresh = tree.begin_file_index(&remote).unwrap();
            assert!(tree.finish_file_index(refresh, remote.clone(), FileList::Failed));
            let FileList::Ready(kept) = tree.file_list(&remote) else {
                panic!("lost usable files")
            };
            assert!(Arc::ptr_eq(&index, &kept));
        });
    }

    #[test]
    fn browsing_home_is_not_a_search_fallback() {
        let home = PathBuf::from("/home/me");
        assert_eq!(
            browsable_roots(Vec::new(), Some(home.clone())),
            vec![home.clone()]
        );
        assert!(searchable_roots(Vec::new(), Some(&home)).is_empty());
        assert!(searchable_roots(vec![home.clone()], Some(&home)).is_empty());
    }
}
