use super::*;
use gpui_kit::{ClipboardEntry, ClipboardItem, Global, Modifiers};

/// The clipboard owns the source connection, not the currently focused tab.
/// Matching the actual clipboard item retires this payload after external copy.
struct FileClipboard {
    item: ClipboardItem,
    source: SharedHost,
    paths: Vec<PathBuf>,
}
impl Global for FileClipboard {}

impl FileTree {
    pub(super) fn select(&mut self, path: &Path, modifiers: Modifiers, cx: &mut Context<Self>) {
        let host = self.view.host;
        let roots = self.roots.clone();
        let expanded = self.expanded.clone();
        let searching = self.view.searching;
        let rows = self.logical_rows(host, &roots, &expanded, searching);
        selection::click(
            &mut self.selected,
            host,
            path,
            selection::primary_modifier(modifiers),
            modifiers.shift,
            &rows,
        );
        cx.emit(FileTreeEvent::ViewStateChanged);
        cx.notify();
    }

    pub(super) fn targets(&self, allow_roots: bool) -> Vec<PathBuf> {
        self.selected
            .targets(self.view.host, &self.roots, allow_roots)
    }

    pub(super) fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let paths = self.targets(true);
        if paths.is_empty() {
            return;
        }
        let Some(source) = self.active_host(cx) else {
            return;
        };
        Self::copy_file_paths(source, paths, cx);
    }

    pub(crate) fn copy_file_paths(source: SharedHost, paths: Vec<PathBuf>, cx: &mut App) {
        let item = if source.id().is_local() && !cfg!(windows) {
            ClipboardItem {
                entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                    paths.clone().into(),
                ))],
            }
        } else {
            // GPUI's Windows writer drops ExternalPaths. Use its supported
            // metadata-bearing string there, retaining the exact source Host.
            // Remote paths must never become local ExternalPaths either.
            ClipboardItem::new_string_with_metadata(
                paths
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n"),
                format!("lyrel-file-copy:{}", uuid::Uuid::new_v4()),
            )
        };
        cx.set_global(FileClipboard {
            item: item.clone(),
            source,
            paths,
        });
        cx.write_to_clipboard(item);
    }

    pub(crate) fn clipboard(cx: &App) -> Option<(SharedHost, Vec<PathBuf>)> {
        let item = cx.read_from_clipboard()?;
        if let Some(payload) = cx.try_global::<FileClipboard>()
            && item == payload.item
        {
            return Some((payload.source.clone(), payload.paths.clone()));
        }
        let paths: Vec<_> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect();
        if paths.is_empty() {
            return None;
        }
        Some((HostRegistry::lookup(cx, HostId::LOCAL)?, paths))
    }

    pub(crate) fn can_paste(cx: &App) -> bool {
        Self::clipboard(cx).is_some()
    }

    pub(super) fn paste(
        &mut self,
        target: Option<(PathBuf, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((source, sources)) = Self::clipboard(cx) else {
            return;
        };
        let Some(destination) = self.active_host(cx) else {
            return;
        };
        let host = destination.id();
        let target = target.or_else(|| {
            let path = self.selected.cursor(host)?;
            let is_dir = self.roots.iter().any(|root| root == path)
                || self.children.get(host, path).is_some()
                || path
                    .parent()
                    .and_then(|parent| self.children.get(host, parent))
                    .is_some_and(|entries| {
                        entries
                            .iter()
                            .any(|entry| entry.path == path && entry.is_dir)
                    });
            Some((path.to_path_buf(), is_dir))
        });
        let dir = match target {
            Some((path, true)) => path,
            Some((path, false)) => match path.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return,
            },
            None => match self.roots.first().cloned() {
                Some(root) => root,
                None => return,
            },
        };
        self.copy_between(
            crate::ui::file_tree::CopyEnds {
                from: source,
                to: destination,
            },
            sources,
            dir,
            false,
            window,
            cx,
        );
    }

    pub(super) fn command_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = ev.keystroke.modifiers;
        let primary = selection::primary_modifier(modifiers);
        if primary && !modifiers.alt && !modifiers.shift {
            match ev.keystroke.key.as_str() {
                "c" => {
                    self.copy_selected(cx);
                    return true;
                }
                "v" => {
                    self.paste(None, window, cx);
                    return true;
                }
                "a" => {
                    let host = self.view.host;
                    let roots = self.roots.clone();
                    let expanded = self.expanded.clone();
                    let searching = self.view.searching;
                    let rows = self.logical_rows(host, &roots, &expanded, searching);
                    self.selected.clear();
                    for row in rows.iter() {
                        if row.note.is_none() && !row.is_root {
                            selection::click(
                                &mut self.selected,
                                host,
                                &row.entry.path,
                                true,
                                false,
                                &rows,
                            );
                        }
                    }
                    cx.emit(FileTreeEvent::ViewStateChanged);
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }
        let delete = if cfg!(target_os = "macos") {
            primary && ev.keystroke.key == "backspace"
        } else {
            !primary && !modifiers.alt && !modifiers.shift && ev.keystroke.key == "delete"
        };
        if delete {
            self.delete_selected(window, cx);
            return true;
        }
        false
    }

    pub(super) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets(false);
        if targets.is_empty() {
            return;
        }
        // Snapshot the exact host and targets before the prompt. Changing tab
        // or reconnecting while it is open cannot retarget a confirmed delete.
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let id = host.id();
        let title = if targets.len() == 1 {
            t_fmt(
                L10nKey::FileTreeDeleteTitle,
                &[("name", &targets[0].display().to_string())],
            )
        } else {
            t_fmt(
                L10nKey::FileTreeDeleteManyTitle,
                &[("n", &targets.len().to_string())],
            )
        };
        let detail = t_fmt(
            L10nKey::FileTreeDeleteSelectionBody,
            &[
                ("host", &self.machine_label()),
                (
                    "targets",
                    &targets
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ],
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &crate::ui::confirm_answers(t(L10nKey::Delete), t(L10nKey::Cancel)),
            cx,
        );
        // Snapshot the adapter identity with the targets: a confirmed delete
        // must not land on the connection that replaced the one the prompt
        // named.
        let authority = self.read_authority(&host);
        cx.spawn_in(window, async move |tree, cx| {
            let Ok(0) = answer.await else {
                return;
            };
            let _ = tree.update_in(cx, |tree, window, cx| {
                for path in targets {
                    let Some(parent) = path.parent().map(Path::to_path_buf) else {
                        continue;
                    };
                    // The prompt can outlive the adapter it named. Check the
                    // saved authority before the optimistic row leaves the
                    // cache and before the remove is dispatched: either one
                    // alone still mutates the connection that replaced the
                    // one the user confirmed the delete on.
                    if !authority.is_current(tree, cx) {
                        continue;
                    }
                    let authority = authority.clone();
                    let row = TreeEntry {
                        name: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        is_dir: tree
                            .children
                            .get(id, &parent)
                            .and_then(|entries| entries.iter().find(|entry| entry.path == path))
                            .is_some_and(|entry| entry.is_dir),
                        path: path.clone(),
                        ignored: false,
                    };
                    let rollback = tree.optimistic(id, &parent, &TreeWrite::Delete, &row);
                    let target = path.clone();
                    let host = host.clone();
                    HostOps::run_in(
                        host,
                        window,
                        cx,
                        move |host| {
                            let canonical = host
                                .canonicalize(&target)
                                .unwrap_or_else(|_| target.clone());
                            let meta = host.stat(&target)?;
                            host.remove(&target, meta.is_dir).map(|()| canonical)
                        },
                        move |tree: &mut FileTree,
                              result: std::io::Result<PathBuf>,
                              _window,
                              cx| {
                            if !authority.is_current(tree, cx) {
                                return;
                            }
                            match tree.finish_write(id, &parent, result, rollback) {
                                Ok(removed) => {
                                    tree.selected.remove_tree(id, &path);
                                    cx.emit(FileTreeEvent::ViewStateChanged);
                                    cx.emit(FileTreeEvent::PathRemoved {
                                        host: id,
                                        path: removed,
                                    });
                                }
                                Err(error) => cx.emit(FileTreeEvent::OperationFailed {
                                    title: t_fmt(
                                        L10nKey::FileTreeDeleteFailed,
                                        &[("name", &path.display().to_string())],
                                    ),
                                    error,
                                }),
                            }
                            cx.notify();
                        },
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The machine label for copy that has to name the host it is about to
    /// change. The app knows the workspace's wording and passes it in with the
    /// view; the tree only reads it.
    fn machine_label(&self) -> String {
        self.view.machine_label.to_string()
    }
}
