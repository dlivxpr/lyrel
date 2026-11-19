//! The thin shell between the standalone `FileTree` entity and the app.
//!
//! Everything file-tree-internal — caches, watcher debounce, searches, inline
//! operations, rendering — lives in `ui::file_tree`. What stays here is the
//! tab-state wiring the entity cannot do without the app: resolving roots
//! from the panes' working directories, mirroring the tree's view state into
//! the active tab's code record, and answering the cross-panel intents the
//! tree announces through `FileTreeEvent`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ui::file_tree::lifecycle::browsable_roots;
use crate::ui::file_tree::{Decorations, FileTree, FileTreeEvent, shell_quote_for};
use crate::ui::notification_history::NotificationWindowExt as _;
use gpui_kit::{Context, Entity, Subscription, Window};
use lyrel_ui_base::host_ops::{HostId, SharedHost};
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use crate::ui::app::LyrelApp;

impl LyrelApp {
    /// The window-scoped subscription to the file tree's intents, installed at
    /// bootstrap next to the other entity watches.
    pub(crate) fn watch_file_tree(
        file_tree: &Entity<FileTree>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(file_tree, window, |app, _, event, window, cx| {
            app.on_file_tree_event(event, window, cx);
        })
    }

    pub(crate) fn on_file_tree_event(
        &mut self,
        event: &FileTreeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FileTreeEvent::OpenFile { path } => self.open_file_in_editor(path, window, cx),
            FileTreeEvent::PathMoved { host, from, to } => {
                self.editor_path_moved(*host, from, to, cx);
            }
            FileTreeEvent::PathRemoved { host, path } => {
                for tab in &mut self.tabs {
                    if let Some(code) = tab.code.as_deref_mut() {
                        code.selected.remove_tree(*host, path);
                    }
                }
                self.editor_path_removed(*host, path, cx);
            }
            FileTreeEvent::FsEvents { host, paths } => {
                self.file_tree_apply_fs_events(*host, paths, cx);
            }
            FileTreeEvent::CdHere { dir } => self.file_tree_cd(dir, window, cx),
            FileTreeEvent::AttachToAgent { path } => self.attach_path_to_agent(path, "", cx),
            FileTreeEvent::PinFolder { dir } => {
                self.pin_folder(dir.clone(), cx);
            }
            FileTreeEvent::InsertPath { path } => self.file_tree_insert_path(path, window, cx),
            FileTreeEvent::ClearSearch => {
                self.file_search
                    .update(cx, |st, cx| st.set_value("", window, cx));
                cx.notify();
            }
            FileTreeEvent::HostRebound => self.panel_search.host_changed(),
            FileTreeEvent::ViewStateChanged => {
                self.sync_file_tree_tab_view(cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn active_host(&self, cx: &gpui_kit::App) -> Option<SharedHost> {
        HostRegistry::lookup(cx, self.spawn_host(cx))
    }

    /// All file readers share the watch's adapter identity, including when the
    /// Files tab is hidden. Synchronize before inspecting a cache or dispatching.
    pub(crate) fn file_tree_sync_host(&mut self, host: &SharedHost, cx: &mut Context<Self>) {
        if self.file_tree.update(cx, |tree, _| tree.sync_host(host)) {
            self.panel_search.host_changed();
            cx.notify();
        }
    }

    pub(crate) fn file_tree_host_rebound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.view()
            && view.read(cx).tab() == crate::ui::search::SearchTab::Files
        {
            // refresh_file_index synchronizes adapter authority before any
            // cache lookup. Hidden consumers start no work at reconnect time.
            self.refresh_search_files(window, cx);
        }
    }

    pub(crate) fn file_tree_refresh_roots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.spawn_host(cx);
        let Some((host, mut roots)) = (if id.is_local() {
            self.project_roots(cx)
        } else {
            // Browsing follows the foreground pane; search keeps project roots.
            self.active_host(cx).map(|host| {
                let cwd = self
                    .tabs
                    .get(self.tabs.active_index())
                    .and_then(|tab| tab.detail_pane(window, cx))
                    .and_then(|pane| {
                        let pane = pane.read(cx);
                        (pane.host_id() == id && pane.remote_context().is_none())
                            .then(|| pane.files_cwd())
                            .flatten()
                    });
                (host, cwd.into_iter().collect())
            })
        }) else {
            return;
        };
        let home = (roots.is_empty() && id.is_local())
            .then(|| std::env::var_os("HOME").map(PathBuf::from))
            .flatten();
        roots = browsable_roots(roots, home);
        // The mirror record must exist before the tree announces the roots it
        // changed, so the ViewStateChanged handler has somewhere to write.
        let _ = self.tab_code_mut_or_init();
        self.file_tree.update(cx, |tree, cx| {
            tree.set_roots(roots, cx);
        });
        self.sync_file_tree_tab_view(cx);
        self.file_tree
            .update(cx, |tree, cx| tree.sync_watch_to_view(host, cx));
    }

    /// The project directories behind the active tab, on the host they live
    /// on: each pane's repository root, or its directory where it is not in
    /// one. `None` while a root is still being asked for (the ask is sent from
    /// here); an empty list where no pane has a directory on this host.
    ///
    /// Search uses these project roots even when remote browsing follows the
    /// foreground pane's subdirectory. Only local browsing falls back to the
    /// home directory — a folder to browse is harmless, a search of all of it
    /// is not.
    pub(crate) fn project_roots(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(SharedHost, Vec<PathBuf>)> {
        let host = self.active_host(cx)?;
        self.file_tree_sync_host(&host, cx);
        let cwds = self.file_project_cwds(cx);
        let roots = self
            .file_tree
            .update(cx, |tree, cx| tree.resolve_project_roots(&host, &cwds, cx))?;
        Some((host, roots))
    }

    pub(crate) fn file_project_cwds(&self, cx: &gpui_kit::App) -> Vec<PathBuf> {
        let id = self.spawn_host(cx);
        let leaves = match self.tabs.get(self.tabs.active_index()) {
            Some(tab) => tab.pane.terminals(),
            None => Vec::new(),
        };
        // `files_cwd`, not `cwd`: a pane running an agent that moved into a
        // git worktree keeps its kernel cwd back at the launch directory, so
        // the raw process cwd would root the tree in the wrong checkout — and
        // in the wrong one *visibly*, since the cwd row directly above this
        // tree already follows the agent. It is also what spells a WSL pane's
        // POSIX cwd through the distro's share, which `host` can read (#896).
        leaves
            .iter()
            .filter(|leaf| leaf.read(cx).host_id() == id)
            .filter_map(|leaf| leaf.read(cx).files_cwd())
            .collect()
    }

    pub(crate) fn file_tree_on_screen(&self, cx: &gpui_kit::App) -> bool {
        self.right_panel_open(cx)
            && self.right_panel_tab == lyrel_core::core::config::RightPanelTab::Files
            && self.sftp_panel.open_pane_id.is_none()
    }

    pub(crate) fn file_tree_query(&self, cx: &gpui_kit::App) -> String {
        self.file_search.read(cx).value().trim().to_lowercase()
    }

    pub(crate) fn file_tree_searching(&self, cx: &gpui_kit::App) -> bool {
        !self.file_tree_query(cx).is_empty()
    }

    pub(crate) fn file_tree_listings_on_screen(&self, cx: &gpui_kit::App) -> bool {
        self.file_tree_on_screen(cx) && !self.file_tree_searching(cx)
    }

    /// Apply a debounced, instance-verified filesystem batch: announce it to
    /// the SCM cache, then let the tree decide what to invalidate and reload.
    pub(crate) fn file_tree_apply_fs_events(
        &mut self,
        host: HostId,
        paths: &HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        log::debug!(
            target: "lyrel::file_tree",
            "fs events on host {host:?}: {:?}",
            paths.iter().take(8).collect::<Vec<_>>()
        );
        let on_screen = self.file_tree_on_screen(cx);
        let listings_on_screen = self.file_tree_listings_on_screen(cx);
        // Working-tree edits the source control cache has no other way to hear
        // about. Anything under `.git` is skipped: the repository has its own
        // watch, and routing it through here would only double the events that
        // land in one debounce window.
        let mut announced: HashSet<&Path> = HashSet::new();
        for path in paths {
            if path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            let Some(dir) = path.parent() else { continue };
            if announced.insert(dir) {
                self.scm_invalidate_cwd(host, dir, cx);
            }
        }
        let active = self.spawn_host(cx);
        let refresh = self.file_tree.update(cx, |tree, cx| {
            tree.apply_fs_events(host, paths, on_screen, listings_on_screen, active, cx)
        });
        if refresh.notify {
            cx.notify();
        }
    }

    pub(crate) fn file_tree_toggle_expand(&mut self, dir: &Path, cx: &mut Context<Self>) {
        self.file_tree
            .update(cx, |tree, cx| tree.toggle_expand(dir, cx));
    }

    pub(crate) fn file_tree_reveal_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        self.file_tree
            .update(cx, |tree, cx| tree.reveal_path(path, cx))
    }

    /// [`Self::file_tree_reveal_path`], with the panel brought up to show it.
    /// For a link to a directory, where the tree is the whole answer and
    /// revealing it behind a closed panel would be the click doing nothing.
    pub(crate) fn file_tree_show(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .file_tree
            .update(cx, |tree, cx| tree.reveal_path(path, cx))
        {
            // Outside every root the tree has nothing to show, and a panel
            // that opened onto the wrong place would be worse than none. The
            // desktop can take it from here — unless the directory is on
            // another machine, where handing the name to a local file manager
            // would open whatever this one keeps at that path, or nothing.
            if self.can_spawn_locally(cx) {
                // The OS association can fail to spawn like any other opener
                // (#542): say so with the same words a failed file link uses.
                if let Err(e) = lyrel_ui_base::desktop_file::open_file_path(path) {
                    log::warn!("failed to open {}: {e}", path.display());
                    window.notify_app(
                        lyrel_ui_base::host_ops::failure(
                            t_fmt(
                                L10nKey::LinkFileOpenFailed,
                                &[
                                    ("path", &path.display().to_string()),
                                    ("error", &e.to_string()),
                                ],
                            ),
                            &e,
                        ),
                        cx,
                    );
                }
            } else {
                window.notify_app(
                    t_fmt(
                        L10nKey::LinkDirOutsideTree,
                        &[("path", &path.display().to_string())],
                    ),
                    cx,
                );
            }
            return;
        }
        if !self.right_panel_open(cx) {
            self.toggle_right_panel(cx);
        }
        self.set_right_panel_tab(lyrel_core::core::config::RightPanelTab::Files, window, cx);
    }

    /// Types `@path` into the running agent's prompt, relative to the tree's
    /// root when the file is under one. `suffix` goes straight after the
    /// path — the editor adds the selected lines as `#L3-9`.
    pub(crate) fn attach_path_to_agent(
        &mut self,
        path: &Path,
        suffix: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.agent_target_leaf(cx) else {
            lyrel_terminal::desktop_notification::notify_desktop(
                Some("lyrel"),
                t(L10nKey::AppNoRunningCodingAgent),
            );
            return;
        };
        let rel = self
            .file_tree
            .read(cx)
            .path_under_root(path)
            .unwrap_or_else(|| path.to_path_buf());
        target.update(cx, |view, cx| {
            view.paste(format!("@{}{suffix} ", rel.display()), cx);
        });
    }

    pub(crate) fn file_tree_copy_between(
        &mut self,
        source: SharedHost,
        host: SharedHost,
        sources: Vec<PathBuf>,
        dir: PathBuf,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_tree.update(cx, |tree, cx| {
            tree.copy_between(source, host, sources, dir, overwrite, window, cx);
        });
    }

    pub(crate) fn file_tree_drop_paths(
        &mut self,
        sources: Vec<PathBuf>,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_tree.update(cx, |tree, cx| {
            tree.drop_paths(sources, dir, window, cx);
        });
    }

    /// Ask each root for a fresh status and take the index it already holds.
    ///
    /// The `Arc` is cloned here, outside the row loop: a tree can be thousands
    /// of rows and every one of them wants the same index. `scm_refresh` is
    /// idempotent and drops a probe that is already running or already current,
    /// which is what makes it safe from a render.
    pub(crate) fn file_tree_decorations(
        &mut self,
        host: Option<&SharedHost>,
        host_id: HostId,
        roots: &[PathBuf],
        cx: &mut Context<Self>,
    ) -> Decorations {
        let mut decor: Decorations = Vec::new();
        for root in roots {
            if let Some(host) = host {
                self.scm_refresh(host.clone(), root.clone(), cx);
            }
            if let Some(index) = crate::ui::scm_snapshot::index_of(cx, host_id, root) {
                decor.push((root.clone(), index));
            }
        }
        crate::ui::file_tree::order_innermost_first(&mut decor);
        decor
    }

    /// Pull the tree's live view state into the active tab's code record, the
    /// way the editor's save root and the SCM panel read it.
    pub(crate) fn sync_file_tree_tab_view(&mut self, cx: &mut Context<Self>) {
        let active = self.tabs.active_index();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        let Some(code) = tab.code.as_deref_mut() else {
            return;
        };
        let (roots, expanded, selected) = {
            let tree = self.file_tree.read(cx);
            (
                tree.roots().to_vec(),
                tree.expanded().clone(),
                tree.selected().clone(),
            )
        };
        code.roots = roots;
        code.expanded = expanded;
        code.selected = selected;
    }

    /// Adopt the active tab's stored view state into the tree, so per-tab
    /// expansion and selection survive a tab switch.
    pub(crate) fn seed_file_tree_tab_view(&mut self, cx: &mut Context<Self>) {
        let active = self.tabs.active_index();
        let view = self
            .tabs
            .get(active)
            .and_then(|tab| tab.code.as_deref())
            .map(|code| {
                (
                    code.roots.clone(),
                    code.expanded.clone(),
                    code.selected.clone(),
                )
            });
        self.file_tree.update(cx, |tree, _| {
            tree.seed_tab_view(view);
        });
    }

    fn file_tree_cd(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(leaf) = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|t| t.pane.focused_or_first(window, cx))
        else {
            return;
        };
        let program = leaf.read(cx).shell_spec().map(|spec| spec.program);
        leaf.read(cx)
            .run_command_line(&format!("cd {}", shell_quote_for(dir, program.as_deref())));
        self.focus_active(window, cx);
    }

    fn file_tree_insert_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(leaf) = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|t| t.pane.focused_or_first(window, cx))
        {
            leaf.update(cx, |view, cx| {
                let program = view.shell_spec().map(|spec| spec.program);
                view.paste(shell_quote_for(path, program.as_deref()), cx);
            });
        }
    }
}
