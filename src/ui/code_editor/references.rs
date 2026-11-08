//! Workbench routing for read-only Markdown references. All lookups use the
//! document's captured Host, never whichever terminal becomes active later.
use super::*;

impl LyrelApp {
    fn reference_open_target(
        &self,
        id: BufferId,
        revision: u64,
        cx: &gpui_kit::App,
    ) -> Option<buffers::OpenTarget> {
        Some(buffers::OpenTarget {
            workspace: self.workspace,
            tab: self.tabs.get(self.tabs.active_index())?.id(),
            group: Some(self.tab_code()?.group),
            window_host: self.spawn_host(cx),
            origin: Some((id, revision)),
            origin_epoch: self.editor_resource_epoch(id, cx),
        })
    }
    fn reference_roots(
        &mut self,
        path: &Path,
        host: HostId,
        cx: &mut Context<Self>,
    ) -> Vec<PathBuf> {
        let mut roots = if host == self.spawn_host(cx) {
            self.project_roots(cx)
                .filter(|(owner, _)| owner.id() == host)
                .map(|(_, roots)| roots)
                .unwrap_or_else(|| self.file_project_cwds(cx))
        } else {
            Vec::new()
        };
        if !roots.iter().any(|root| path.starts_with(root)) {
            roots.clear();
            if let Some(parent) = path.parent() {
                roots.push(parent.to_path_buf());
            }
        }
        roots
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn editor_query_references(
        &mut self,
        id: BufferId,
        presentation: gpui_kit::WeakEntity<markdown::SourcePresentation>,
        generation: u64,
        revision: u64,
        resource_epoch: Option<(HostId, u64)>,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        query: String,
        anchors: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.editor.buffers.get(id) else {
            return;
        };
        if file.untitled.is_some()
            || self.active_buffer().is_none_or(|active| active.id() != id)
            || self.editor_resource_epoch(id, cx) != resource_epoch
            || self
                .active_editor_input()
                .is_none_or(|input| input.markdown().is_none_or(|view| *view != presentation))
        {
            return;
        }
        let host = file.host.clone();
        let path = file.path.clone();
        let current = file.input.read(cx).text().clone();
        let roots = self.reference_roots(&path, host.id(), cx);
        let Some(target) = self.reference_open_target(id, revision, cx) else {
            return;
        };
        let lease_path = path.clone();
        let lease_host = host.id();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| {
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(Vec::new());
                }
                let current = (anchors || query.contains('#')).then(|| current.to_string());
                markdown::links::references::query(
                    host,
                    &path,
                    &roots,
                    &query,
                    anchors,
                    current.as_deref(),
                    &cancelled,
                )
            },
            move |app, result, window, cx| {
                if !app.editor_open_target_valid(target, cx)
                    || app.active_buffer().is_none_or(|file| {
                        file.id() != id || file.host.id() != lease_host || file.path != lease_path
                    })
                    || app.active_editor_input().is_none_or(|input| {
                        input.markdown().is_none_or(|view| *view != presentation)
                    })
                {
                    if let Some(view) = presentation.upgrade() {
                        view.update(cx, |view, cx| {
                            view.discard_reference_results(generation, cx)
                        });
                    }
                    return;
                }
                if let Some(view) = presentation.upgrade() {
                    view.update(cx, |view, cx| {
                        view.install_reference_results(generation, result, window, cx)
                    });
                }
            },
        );
    }
    #[allow(clippy::too_many_arguments)]
    fn editor_follow_file_fragment(
        &mut self,
        id: BufferId,
        revision: u64,
        host: SharedHost,
        path: PathBuf,
        fragment: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.reference_open_target(id, revision, cx) else {
            return;
        };
        let origin_path = self.editor.buffers.get(id).map(|file| file.path.clone());
        let existing = self
            .editor
            .buffers
            .at(host.id(), &path)
            .and_then(|id| self.editor.buffers.get(id));
        let snapshot = existing.map(|file| (file.id(), file.input.read(cx).text().clone()));
        let current = snapshot.as_ref().map(|(_, text)| text.clone());
        let existing_revision = existing
            .and_then(|file| file.input.markdown())
            .map(|view| view.read(cx).document().read(cx).revision());
        let open_host = host.clone();
        let open_path = path.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| {
                markdown::links::references::anchor_position(
                    host,
                    &path,
                    &fragment,
                    current.map(|source| source.to_string()),
                )
            },
            move |app, result, window, cx| {
                if !app.editor_open_target_valid(target, cx)
                    || app.editor.buffers.get(id).is_none_or(|file| {
                        Some(&file.path) != origin_path.as_ref() || file.host.id() != open_host.id()
                    })
                {
                    return;
                }
                if let Some((id, _)) = &snapshot {
                    if app
                        .editor
                        .buffers
                        .get(*id)
                        .and_then(|file| file.input.markdown())
                        .map(|view| view.read(cx).document().read(cx).revision())
                        != existing_revision
                    {
                        return;
                    }
                }
                match result {
                    Ok(position) => app.editor_open_request(
                        open_host,
                        &open_path,
                        Some(position),
                        Some((id, revision)),
                        window,
                        cx,
                    ),
                    Err(reason) => window.notify_app(
                        t_fmt(L10nKey::MarkdownLinkOpenFailed, &[("reason", t(reason))]),
                        cx,
                    ),
                }
            },
        );
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn editor_resolve_reference_file(
        &mut self,
        id: BufferId,
        revision: u64,
        host: SharedHost,
        path: PathBuf,
        fragment: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.reference_open_target(id, revision, cx) else {
            return;
        };
        let origin_path = self.editor.buffers.get(id).map(|file| file.path.clone());
        let open_host = host.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| markdown::links::references::file_path(host, &path),
            move |app, result, window, cx| {
                if !app.editor_open_target_valid(target, cx)
                    || app.editor.buffers.get(id).is_none_or(|file| {
                        Some(&file.path) != origin_path.as_ref() || file.host.id() != open_host.id()
                    })
                {
                    return;
                }
                match result {
                    Ok(path) => {
                        if let Some(fragment) = fragment {
                            app.editor_follow_file_fragment(
                                id, revision, open_host, path, fragment, window, cx,
                            );
                        } else {
                            app.editor_open_request(
                                open_host,
                                &path,
                                None,
                                Some((id, revision)),
                                window,
                                cx,
                            );
                        }
                    }
                    Err(reason) => window.notify_app(
                        t_fmt(L10nKey::MarkdownLinkOpenFailed, &[("reason", t(reason))]),
                        cx,
                    ),
                }
            },
        );
    }
}
