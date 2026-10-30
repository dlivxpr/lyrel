use super::*;
use crate::ui::host_ops::HostOps;

pub(super) fn primary(modifiers: gpui_kit::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}

impl LyrelApp {
    pub(super) fn sftp_select_entry(
        &mut self,
        entry: &SftpEntry,
        modifiers: gpui_kit::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sftp_panel.focus_handle.focus(window, cx);
        let filter = self.sftp_panel.filter_input.read(cx).value();
        let rows = sorted_filtered_entries(&self.sftp_panel.entries, &filter);
        let range = modifiers
            .shift
            .then(|| {
                let anchor = self.sftp_panel.selection_anchor.as_ref()?;
                Some((
                    rows.iter().position(|e| &e.name == anchor)?,
                    rows.iter().position(|e| e.name == entry.name)?,
                ))
            })
            .flatten();
        if let Some((a, b)) = range {
            if !primary(modifiers) {
                self.sftp_panel.selected.clear();
            }
            self.sftp_panel
                .selected
                .extend(rows[a.min(b)..=a.max(b)].iter().map(|e| e.name.clone()));
        } else if primary(modifiers) {
            if !self.sftp_panel.selected.remove(&entry.name) {
                self.sftp_panel.selected.insert(entry.name.clone());
            }
            self.sftp_panel.selection_anchor = Some(entry.name.clone());
        } else {
            self.sftp_panel.selected.clear();
            self.sftp_panel.selected.insert(entry.name.clone());
            self.sftp_panel.selection_anchor = Some(entry.name.clone());
        }
        cx.notify();
    }

    fn sftp_selection_paths(&self) -> Vec<PathBuf> {
        self.sftp_panel
            .entries
            .iter()
            .filter(|entry| {
                self.sftp_panel.selected.contains(&entry.name) && safe_local_name(&entry.name)
            })
            .map(|entry| PathBuf::from(remote_join(&self.sftp_panel.cwd, &entry.name)))
            .collect()
    }

    pub(super) fn sftp_copy_selection(&mut self, cx: &mut Context<Self>) {
        let paths = self.sftp_selection_paths();
        if !paths.is_empty()
            && let Some(host) = self.sftp_editor_host()
        {
            Self::copy_file_paths(host, paths, cx);
        }
    }

    pub(super) fn sftp_paste_selection(
        &mut self,
        into: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((source, paths)) = Self::file_tree_clipboard(cx) else {
            return;
        };
        let Some(destination) = self.sftp_editor_host() else {
            return;
        };
        let dir = PathBuf::from(into.unwrap_or_else(|| self.sftp_panel.cwd.clone()));
        self.file_tree_copy_between(source, destination, paths, dir, false, window, cx);
    }

    pub(super) fn sftp_selection_key(
        &mut self,
        ev: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        let handled = if primary(modifiers) {
            match key {
                "c" => {
                    self.sftp_copy_selection(cx);
                    true
                }
                "v" => {
                    self.sftp_paste_selection(None, window, cx);
                    true
                }
                "a" => {
                    let filter = self.sftp_panel.filter_input.read(cx).value();
                    self.sftp_panel.selected =
                        sorted_filtered_entries(&self.sftp_panel.entries, &filter)
                            .iter()
                            .map(|entry| entry.name.clone())
                            .collect();
                    cx.notify();
                    true
                }
                "backspace" if cfg!(target_os = "macos") => {
                    self.sftp_delete_selection(window, cx);
                    true
                }
                _ => false,
            }
        } else if key == "delete" && !cfg!(target_os = "macos") {
            self.sftp_delete_selection(window, cx);
            true
        } else {
            false
        };
        if handled {
            cx.stop_propagation();
        }
    }

    pub(super) fn sftp_delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.sftp_selection_paths();
        let Some(host) = self.sftp_editor_host() else {
            return;
        };
        if targets.is_empty() {
            return;
        }
        let generation = self.sftp_panel.nav_gen;
        let route = self.sftp_route().connection_key();
        let host_label = self
            .remote_files_host(window, cx)
            .unwrap_or_else(|| route.clone());
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
                ("host", &host_label),
                (
                    "targets",
                    &targets
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ],
        );
        let answer = window.prompt(
            gpui_kit::PromptLevel::Warning,
            &title,
            Some(&detail),
            &crate::ui::confirm_answers(t(L10nKey::Delete), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            let Ok(0) = answer.await else { return };
            let _ = app.update_in(cx, |app, window, cx| {
                // A prompt from a retired pane/navigation cannot act through a
                // replacement SSH connection that happens to reuse its pane ID.
                if app.sftp_panel.nav_gen != generation
                    || app.sftp_route().connection_key() != route
                {
                    return;
                }
                let id = host.id();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |host| {
                        targets
                            .into_iter()
                            .map(|path| {
                                let result = host.remove(&path, true);
                                (path, result)
                            })
                            .collect::<Vec<_>>()
                    },
                    move |app, results, window, cx| {
                        for (path, result) in results {
                            match result {
                                Ok(()) => {
                                    app.editor_path_removed(id, &path, cx);
                                    if app.sftp_panel.nav_gen == generation {
                                        if let Some(name) =
                                            path.file_name().and_then(|name| name.to_str())
                                        {
                                            app.sftp_panel.selected.remove(name);
                                        }
                                    }
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
                        }
                        if app.sftp_panel.nav_gen == generation
                            && app.sftp_route().connection_key() == route
                        {
                            app.sftp_refresh(cx);
                        }
                        cx.notify();
                    },
                );
            });
        })
        .detach();
    }
}
