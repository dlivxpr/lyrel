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

impl LyrelApp {
    pub(super) fn file_tree_select(
        &mut self,
        path: &Path,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let host = self.spawn_host(cx);
        let Some(code) = self.tab_code() else { return };
        let rows = self.file_tree.logical_rows(host, &code.roots, &code.expanded, self.file_tree_searching(cx));
        if let Some(code) = self.tab_code_mut() {
            code.selected.click(
                host,
                path,
                selection::primary_modifier(modifiers),
                modifiers.shift,
                &rows,
            );
        }
        cx.notify();
    }

    pub(super) fn file_tree_targets(&self, cx: &App, allow_roots: bool) -> Vec<PathBuf> {
        let host = self.spawn_host(cx);
        self.tab_code()
            .map(|code| code.selected.targets(host, &code.roots, allow_roots))
            .unwrap_or_default()
    }

    pub(super) fn file_tree_copy_selected(&mut self, cx: &mut Context<Self>) {
        let paths = self.file_tree_targets(cx, true);
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

    pub(crate) fn file_tree_clipboard(cx: &App) -> Option<(SharedHost, Vec<PathBuf>)> {
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

    pub(crate) fn file_tree_can_paste(cx: &App) -> bool {
        Self::file_tree_clipboard(cx).is_some()
    }

    pub(super) fn file_tree_paste(
        &mut self,
        target: Option<(PathBuf, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((source, sources)) = Self::file_tree_clipboard(cx) else {
            return;
        };
        let Some(destination) = self.active_host(cx) else {
            return;
        };
        let host = destination.id();
        let target = target.or_else(|| {
            let code = self.tab_code()?;
            let path = code.selected.cursor(host)?;
            let is_dir = code.roots.iter().any(|root| root == path)
                || self.file_tree.children.get(host, path).is_some()
                || path
                    .parent()
                    .and_then(|parent| self.file_tree.children.get(host, parent))
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
            None => match self.tab_code().and_then(|code| code.roots.first()).cloned() {
                Some(root) => root,
                None => return,
            },
        };
        self.file_tree_copy_between(source, destination, sources, dir, false, window, cx);
    }

    pub(super) fn file_tree_command_key(
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
                    self.file_tree_copy_selected(cx);
                    return true;
                }
                "v" => {
                    self.file_tree_paste(None, window, cx);
                    return true;
                }
                "a" => {
                    let host = self.spawn_host(cx);
                    let Some(code) = self.tab_code() else { return true };
                    let rows = self.file_tree.logical_rows(host, &code.roots, &code.expanded, self.file_tree_searching(cx));
                    if let Some(code) = self.tab_code_mut() {
                        code.selected.clear();
                        for row in rows.iter() {
                            if row.note.is_none() && !row.is_root {
                                code.selected
                                    .click(host, &row.entry.path, true, false, &rows);
                            }
                        }
                    }
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
            self.file_tree_delete_selected(window, cx);
            return true;
        }
        false
    }

    pub(super) fn file_tree_delete_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.file_tree_targets(cx, false);
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
                ("host", &self.remote_machine_label(cx)),
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
        cx.spawn_in(window, async move |app, cx| {
            let Ok(0) = answer.await else {
                return;
            };
            let _ = app.update_in(cx, |app, window, cx| {
                for path in targets {
                    let Some(parent) = path.parent().map(Path::to_path_buf) else {
                        continue;
                    };
                    let row = TreeEntry {
                        name: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        is_dir: app
                            .file_tree
                            .children
                            .get(id, &parent)
                            .and_then(|entries| entries.iter().find(|entry| entry.path == path))
                            .is_some_and(|entry| entry.is_dir),
                        path: path.clone(),
                        ignored: false,
                    };
                    let rollback = app
                        .file_tree
                        .optimistic(id, &parent, &TreeWrite::Delete, &row);
                    let target = path.clone();
                    HostOps::run_in(
                        host.clone(),
                        window,
                        cx,
                        move |host| {
                            let canonical = host
                                .canonicalize(&target)
                                .unwrap_or_else(|_| target.clone());
                            let meta = host.stat(&target)?;
                            host.remove(&target, meta.is_dir).map(|()| canonical)
                        },
                        move |app, result: std::io::Result<PathBuf>, window, cx| {
                            match app.file_tree.finish_write(id, &parent, result, rollback) {
                                Ok(removed) => {
                                    for tab in &mut app.tabs {
                                        if let Some(code) = tab.code.as_deref_mut() {
                                            code.selected.remove_tree(id, &path);
                                        }
                                    }
                                    app.editor_path_removed(id, &removed, cx);
                                }
                                Err(error) => HostOps::notify_err(
                                    window,
                                    cx,
                                    &t_fmt(
                                        L10nKey::FileTreeDeleteFailed,
                                        &[("name", &path.display().to_string())],
                                    ),
                                    &error,
                                ),
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
}
