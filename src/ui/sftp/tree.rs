use super::*;
use std::ops::Deref;

/// A row's remote identity never depends on the currently displayed root.
#[derive(Clone)]
pub(super) struct SftpTarget {
    pub(super) path: String,
    pub(super) entry: SftpEntry,
}

impl Deref for SftpTarget {
    type Target = SftpEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

impl SftpPanelState {
    pub(super) fn clear_tree(&mut self) {
        self.children.clear();
        self.expanded.clear();
        self.child_loads.clear();
    }

    pub(super) fn prune_tree(&mut self) {
        fn collect(
            panel: &SftpPanelState,
            parent: &str,
            entries: &[SftpEntry],
            known: &mut HashSet<String>,
        ) {
            for entry in entries {
                let path = remote_join(parent, &entry.name);
                known.insert(path.clone());
                if let Some(Ok(children)) = panel.children.get(&path) {
                    collect(panel, &path, children, known);
                }
            }
        }
        let mut known = HashSet::new();
        collect(self, &self.cwd, &self.entries, &mut known);
        self.expanded.retain(|path| known.contains(path));
        self.children.retain(|path, _| known.contains(path));
        self.child_loads.retain(|path, _| known.contains(path));
        self.selected.retain(|path| known.contains(path));
    }
    pub(super) fn visible_rows(&self, filter: &str) -> Vec<(String, &SftpEntry, usize)> {
        self.rows(filter, true)
    }

    pub(super) fn all_rows(&self) -> Vec<(String, &SftpEntry, usize)> {
        self.rows("", false)
    }

    fn rows(&self, filter: &str, expanded_only: bool) -> Vec<(String, &SftpEntry, usize)> {
        fn append<'a>(
            panel: &'a SftpPanelState,
            directory: &str,
            entries: &'a [SftpEntry],
            depth: usize,
            filter: &str,
            expanded_only: bool,
            rows: &mut Vec<(String, &'a SftpEntry, usize)>,
        ) {
            for entry in sorted_filtered_entries(entries, "") {
                let path = remote_join(directory, &entry.name);
                let start = rows.len();
                rows.push((path.clone(), entry, depth));
                if (!expanded_only || panel.expanded.contains(&path))
                    && let Some(Ok(children)) = panel.children.get(&path)
                {
                    append(
                        panel,
                        &path,
                        children,
                        depth + 1,
                        filter,
                        expanded_only,
                        rows,
                    );
                }
                if !filter.is_empty()
                    && !entry.name.to_lowercase().contains(filter)
                    && rows.len() == start + 1
                {
                    rows.pop();
                }
            }
        }
        let mut rows = Vec::new();
        append(
            self,
            &self.cwd,
            &self.entries,
            0,
            &filter.to_lowercase(),
            expanded_only,
            &mut rows,
        );
        rows
    }

    pub(super) fn land_children(
        &mut self,
        path: String,
        navigation: u64,
        ticket: u64,
        reply: Result<Vec<SftpEntry>, String>,
    ) -> bool {
        if self.nav_gen != navigation || self.child_loads.get(&path) != Some(&ticket) {
            return false;
        }
        self.child_loads.remove(&path);
        self.children.insert(path, reply);
        self.prune_tree();
        true
    }
}

impl LyrelApp {
    pub(super) fn sftp_toggle_directory(&mut self, path: String, cx: &mut Context<Self>) {
        if self.sftp_panel.expanded.remove(&path) {
            self.sftp_panel.child_loads.remove(&path);
        } else {
            self.sftp_panel.expanded.insert(path.clone());
            self.sftp_load_children(path, cx);
        }
        cx.notify();
    }

    pub(super) fn sftp_refresh_expanded(&mut self, cx: &mut Context<Self>) {
        let paths = self.sftp_panel.expanded.iter().cloned().collect::<Vec<_>>();
        for path in paths {
            self.sftp_load_children(path, cx);
        }
    }

    fn sftp_load_children(&mut self, path: String, cx: &mut Context<Self>) {
        if self.sftp_panel.open_pane_id.is_none() || self.sftp_panel.child_loads.contains_key(&path)
        {
            return;
        }
        self.sftp_panel.child_generation = self.sftp_panel.child_generation.wrapping_add(1);
        let ticket = self.sftp_panel.child_generation;
        let navigation = self.sftp_panel.nav_gen;
        self.sftp_panel.child_loads.insert(path.clone(), ticket);
        let route = self.sftp_route();
        cx.spawn(async move |app, cx| {
            let (path, reply) = cx
                .background_spawn(async move {
                    let reply = route.list(&path);
                    (path, reply)
                })
                .await;
            let _ = app.update(cx, |app, cx| {
                if app
                    .sftp_panel
                    .land_children(path, navigation, ticket, reply)
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window::harness;
    use gpui_kit::TestAppContext;

    fn entry(name: &str, kind: SftpEntryKind) -> SftpEntry {
        SftpEntry {
            name: name.into(),
            kind,
            size: 0,
            mtime: 0,
            permissions: 0o755,
            target_is_dir: false,
        }
    }

    #[gpui_kit::test]
    fn nested_selection_keeps_remote_identity_and_rejects_retired_child_replies(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            let panel = &mut app.sftp_panel;
            panel.open_pane_id = Some(44);
            panel.cwd = "/fixture".into();
            panel.entries = vec![
                entry("first", SftpEntryKind::Dir),
                entry("second", SftpEntryKind::Dir),
            ];
            for parent in ["/fixture/first", "/fixture/second"] {
                panel.expanded.insert(parent.into());
                panel.child_loads.insert(parent.into(), 10);
                assert!(panel.land_children(
                    parent.into(),
                    panel.nav_gen,
                    10,
                    Ok(vec![entry("same.txt", SftpEntryKind::File)])
                ));
            }
            let target = SftpTarget {
                path: "/fixture/second/same.txt".into(),
                entry: entry("same.txt", SftpEntryKind::File),
            };
            app.sftp_select_entry(&target, gpui_kit::Modifiers::default(), window, cx);
            assert_eq!(
                app.sftp_selection_paths(),
                [PathBuf::from("/fixture/second/same.txt")]
            );
            app.sftp_panel.expanded.remove("/fixture/second");
            assert_eq!(
                app.sftp_selection_paths(),
                [PathBuf::from("/fixture/second/same.txt")]
            );
            assert_eq!(app.sftp_panel.visible_rows("").len(), 3);
            let panel = &mut app.sftp_panel;
            panel.child_loads.insert("/fixture/first".into(), 11);
            assert!(!panel.land_children("/fixture/first".into(), panel.nav_gen, 10, Ok(vec![])));
            let retired = panel.nav_gen;
            panel.nav_gen += 1;
            assert!(!panel.land_children("/fixture/first".into(), retired, 11, Ok(vec![])));
            assert_eq!(panel.children["/fixture/first"].as_ref().unwrap().len(), 1);
        });
    }
}
