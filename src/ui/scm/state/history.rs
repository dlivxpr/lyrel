//! History query, selection and detail lifecycle. The window supplies GPUI
//! scheduling; no operation receives mutable access to the workbench.

use super::*;
use crate::ui::app::LyrelApp;
use crate::ui::host_ops::HostOps;
use gpui_kit::{AppContext as _, Context, Focusable as _};
use lyrel_core::core::git::log::{self, GRAPH_PAGE, MAX_GRAPH_COMMITS};

type GraphKey = (RepoKey, u64, GraphScope);

struct GraphLoad {
    key: GraphKey,
    generation: u64,
    requested: usize,
}

struct DetailLoad {
    repo: RepoKey,
    oid: String,
    epoch: u64,
    generation: u64,
    seeded: bool,
}

impl ScmPanelState {
    /// Presentation borrows the retained graph, never a mutable field bag.
    pub(crate) fn graph(&self) -> &GraphState { &self.graph }
    pub(crate) fn detail(&self) -> Option<&CommitDetailView> { self.detail.as_ref() }

    pub(crate) fn toggle_graph(&mut self) -> bool {
        self.graph.expanded = !self.graph.expanded;
        self.graph.expanded
    }

    pub(crate) fn request_more_history(&mut self) {
        self.graph.requested = next_page_request(self.graph.requested);
    }

    pub(crate) fn set_graph_scope(&mut self, scope: GraphScope, cx: &mut Context<LyrelApp>) {
        if self.graph.scope == scope { return }
        self.graph.scope = scope;
        self.graph.requested = GRAPH_PAGE;
        self.graph.page = None;
        self.graph.page_key = None;
        self.invalidate_graph_load();
        cx.notify();
    }

    fn invalidate_graph_load(&mut self) {
        self.graph.generation = self.graph.generation.wrapping_add(1);
        self.graph.loading = false;
        self.graph.loading_key = None;
    }

    pub(super) fn invalidate_history_host(&mut self, host: HostId) {
        if self.graph.page_key.as_ref().is_some_and(|(repo, _, _)| repo.host == host) {
            self.graph.page = None;
            self.graph.page_key = None;
            self.graph.requested = 0;
        }
        if self.graph.loading_key.as_ref().is_some_and(|(repo, _, _, _)| repo.host == host) {
            self.invalidate_graph_load();
        }
        if self.graph.failed_key.as_ref().is_some_and(|(repo, _, _)| repo.host == host) {
            self.graph.failed_key = None;
        }
        if self.detail.as_ref().is_some_and(|detail| detail.repo.host == host) {
            self.detail = None;
            self.detail_generation = self.detail_generation.wrapping_add(1);
        }
    }

    /// Query growth reruns a deterministic prefix, never a sliding --skip.
    /// Repo changes drop the old page before presentation can draw its revs.
    fn begin_graph_load(&mut self, repo: &RepoKey, epoch: u64) -> Option<GraphLoad> {
        let different_repo = self.graph.page_key.as_ref().is_some_and(|(held, _, _)| held != repo)
            || self.graph.loading_key.as_ref().is_some_and(|(held, _, _, _)| held != repo);
        if different_repo {
            self.graph.page = None;
            self.graph.page_key = None;
            self.graph.requested = 0;
        }
        let want = self.graph.requested.clamp(GRAPH_PAGE, MAX_GRAPH_COMMITS);
        let key = (repo.clone(), epoch, self.graph.scope.clone());
        let fresh = self.graph.page_key.as_ref() == Some(&key)
            && self.graph.page.as_ref().is_some_and(|page| page.requested >= want);
        let loading = self.graph.loading_key.as_ref()
            .is_some_and(|(held, at, scope, _)| held == repo && *at == epoch && *scope == self.graph.scope);
        if fresh || loading || self.graph.failed_key.as_ref() == Some(&key) { return None }
        self.invalidate_graph_load();
        self.graph.loading = true;
        self.graph.loading_key = Some((repo.clone(), epoch, self.graph.scope.clone(), self.graph.generation));
        Some(GraphLoad { key, generation: self.graph.generation, requested: want })
    }

    fn apply_graph(&mut self, load: GraphLoad, page: Option<CommitPage>, cx: &App) -> bool {
        let (repo, epoch, scope) = &load.key;
        if !self.graph.loading_key.as_ref().is_some_and(|(held, at, query, generation)|
            held == repo && at == epoch && query == scope && *generation == load.generation)
        {
            return false;
        }
        self.graph.loading = false;
        self.graph.loading_key = None;
        if repo.refresh_epoch(cx) != *epoch || self.active_repo() != Some(repo)
            || HostRegistry::lookup(cx, repo.host).is_none() || self.graph.scope != *scope
        { return true }
        match page {
            Some(page) => {
                self.graph.failed_key = None;
                self.graph.page = Some(Arc::new(page));
                self.graph.page_key = Some(load.key);
            }
            None => self.graph.failed_key = Some(load.key),
        }
        true
    }

    pub(crate) fn load_graph(&mut self, repo: &RepoKey, cx: &mut Context<LyrelApp>) {
        // Read-only global access: mutable global access from render would
        // enqueue observer effects forever, even for an unchanged page.
        let epoch = repo.refresh_epoch(cx);
        // Capture Host only when dispatching, retaining the established
        // no-allocation fast path for already fresh branch reads.
        let Some(load) = self.begin_graph_load(repo, epoch) else { return };
        let Some(context) = repo.context(cx) else {
            self.invalidate_graph_load();
            return;
        };
        let (_, host, _) = context.into_parts();
        let root = repo.root.clone();
        let scope = load.key.2.clone();
        let want = load.requested;
        HostOps::run(host, cx, move |host| log::load_page(host, &root, &scope, want),
            move |this, page, cx| {
                if this.scm.apply_graph(load, page, cx) { cx.notify(); }
            });
    }

    pub(crate) fn graph_visible_rows(&mut self, page: &Arc<CommitPage>, query: Option<&str>) -> Arc<Vec<usize>> {
        if let Some((held_query, held_page, rows)) = &self.graph.filter_cache {
            if held_page.ptr_eq(&Arc::downgrade(page)) && held_query.as_deref() == query {
                return rows.clone();
            }
        }
        let rows = Arc::new(match query {
            None => (0..page.commits.len()).collect(),
            Some(query) => (0..page.commits.len()).filter(|i| matches_query(&page.commits[*i], query)).collect(),
        });
        self.graph.filter_cache = Some((query.map(str::to_string), Arc::downgrade(page), Arc::clone(&rows)));
        rows
    }

    pub(crate) fn graph_query(&self, cx: &App) -> Option<String> {
        let input = self.graph.search.as_ref()?;
        let text = input.read(cx).value().trim().to_lowercase();
        (!text.is_empty()).then_some(text)
    }

    pub(crate) fn toggle_graph_search(&mut self, window: &mut Window, cx: &mut Context<LyrelApp>) {
        if self.graph.search.take().is_some() {
            self.graph.search_sub = None;
            cx.notify();
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::ScmGraphFilterPlaceholder)));
        self.graph.search_sub = Some(cx.subscribe_in(&input, window, |_this, _input, event, _window, cx| {
            if matches!(event, gpui_kit::component::input::InputEvent::Change) { cx.notify(); }
        }));
        let handle = input.read(cx).focus_handle(cx);
        self.graph.search = Some(input);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn open_graph_commit(&mut self, repo: RepoKey, oid: String, seed: Option<Commit>, cx: &mut Context<LyrelApp>) {
        self.graph.selected = Some(oid.clone());
        self.open_detail(repo, oid, seed, cx);
    }

    pub(crate) fn open_detail(&mut self, repo: RepoKey, oid: String, seed: Option<Commit>, cx: &mut Context<LyrelApp>) {
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.detail = Some(CommitDetailView::new(repo, oid, seed));
        cx.notify();
    }

    pub(crate) fn close_detail(&mut self, cx: &mut Context<LyrelApp>) {
        if self.detail.take().is_some() {
            self.detail_generation = self.detail_generation.wrapping_add(1);
            cx.notify();
        }
    }

    pub(crate) fn toggle_detail_body(&mut self, cx: &mut Context<LyrelApp>) {
        if let Some(detail) = &mut self.detail {
            detail.body_expanded = !detail.body_expanded;
            cx.notify();
        }
    }

    fn begin_detail_load(&mut self, cx: &App) -> Option<(DetailLoad, SharedHost)> {
        if self.detail.as_ref().is_some_and(|detail| self.active_repo() != Some(&detail.repo)) {
            self.detail = None;
            self.detail_generation = self.detail_generation.wrapping_add(1);
        }
        let detail = self.detail.as_mut()?;
        if detail.loading || detail.loaded { return None }
        let (repo, host, epoch) = detail.repo.context(cx)?.into_parts();
        detail.loading = true;
        Some((DetailLoad { repo, oid: detail.oid.clone(), epoch, generation: self.detail_generation, seeded: detail.commit.is_some() }, host))
    }

    fn apply_detail(&mut self, load: DetailLoad, commit: Option<Commit>, files: Option<Vec<CommitFile>>, cx: &App) -> bool {
        if self.detail_generation != load.generation { return false }
        let active = self.active_repo() == Some(&load.repo);
        let Some(detail) = self.detail.as_mut().filter(|detail| detail.repo == load.repo && detail.oid == load.oid) else { return false };
        detail.loading = false;
        if !active || load.repo.refresh_epoch(cx) != load.epoch || HostRegistry::lookup(cx, load.repo.host).is_none() { return true }
        detail.loaded = true;
        if let Some(commit) = commit { detail.commit = Some(Arc::new(commit)); }
        match files {
            Some(files) => detail.files = Some(Arc::new(files)),
            None => detail.files_failed = true,
        }
        true
    }

    /// Reconcile selection and schedule once; presentation receives only the
    /// immutable snapshot after this transition. Seeds skip metadata I/O.
    pub(crate) fn refresh_detail(&mut self, cx: &mut Context<LyrelApp>) {
        let Some((load, host)) = self.begin_detail_load(cx) else { return };
        let root = load.repo.root.clone();
        let oid = load.oid.clone();
        let seeded = load.seeded;
        HostOps::run(host, cx, move |host| {
            let commit = (!seeded).then(|| log::load_commit(host, &root, &oid)).flatten();
            (commit, log::commit_files(host, &root, &oid))
        }, move |this, (commit, files), cx| {
            if this.scm.apply_detail(load, commit, files, cx) { cx.notify(); }
        });
    }

    pub(crate) fn begin_branch_at(&mut self, repo: RepoKey, rev: String, tab: usize, window: &mut Window, cx: &mut Context<LyrelApp>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::ScmCreateBranchHere)));
        let handle = input.read(cx).focus_handle(cx);
        self.graph.naming = Some((input, rev));
        self.select_repository(repo, tab);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn cancel_branch_at(&mut self) { self.graph.naming = None; }

    pub(crate) fn submit_branch_at(&mut self, cx: &App) -> Option<lyrel_core::core::git::ops::GitOp> {
        let (input, rev) = self.graph.naming.take()?;
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() { return None }
        Some(lyrel_core::core::git::ops::GitOp::CreateBranch { name, start: Some(rev), checkout: false })
    }
}

/// A deterministic prefix query cannot grow beyond the loader's cap.
fn next_page_request(requested: usize) -> usize {
    requested.max(GRAPH_PAGE).saturating_add(GRAPH_PAGE).min(MAX_GRAPH_COMMITS)
}

/// Only text visible in a graph row participates in its search.
fn matches_query(commit: &Commit, query: &str) -> bool {
    commit.summary.to_lowercase().contains(query)
        || commit.author.name.to_lowercase().contains(query)
        || commit.oid.starts_with(query)
}

#[cfg(test)]
mod tests;
