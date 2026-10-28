//! Everything the source control panel remembers between frames.
//!
//! All of it is app-level, hanging off `LyrelApp.scm` rather than off a tab.
//! The panel has exactly one instance per window — the same model
//! `RightPanelState.diff_cwd` already uses. `Tab.diff_overlay` and `Tab.code`
//! are per-tab because they are full-screen overlays that belong to a tab; a
//! side panel does not.
//!
//! The one thing that must survive everything is the commit draft, so it is
//! keyed by repository rather than by tab or pane: a working tree has one
//! pending message, no matter how many panes are looking at it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::{App, AppContext as _, Entity, Window};
use lyrel_core::core::git::log::{Commit, CommitFile, CommitPage, GraphScope};
use lyrel_core::core::git::status::{HeadState, WorkingTreeStatus};

use crate::ui::host_ops::{HostId, SharedHost};
use crate::ui::host_registry::HostRegistry;
use crate::ui::i18n::{L10nKey, t};

mod branches;
mod history;

pub(crate) use branches::BranchInput;

/// Which working tree a piece of state belongs to.
///
/// The host is part of the key because the same path can exist on this machine
/// and on three different remotes at once, and they are unrelated repositories.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct RepoKey {
    pub(crate) host: HostId,
    pub(crate) root: PathBuf,
}

impl RepoKey {
    /// Capture one repository/Host/refresh identity before dispatch. Never pair
    /// a selected root with the active pane's fallback Host.
    pub(crate) fn context(&self, cx: &App) -> Option<RepositoryContext> {
        let host = HostRegistry::lookup(cx, self.host)?;
        Some(RepositoryContext {
            repo: self.clone(),
            host,
            epoch: self.refresh_epoch(cx),
        })
    }

    /// Inspect refresh identity without cloning a root or Host on a render
    /// that already has a fresh branch/graph result.
    pub(crate) fn refresh_epoch(&self, cx: &App) -> u64 {
        cx.try_global::<crate::terminal::git_data::ScmData>()
            .map_or(0, |data| data.epoch(self.host, &self.root))
    }
}

/// A dispatch snapshot, not another owner or a live selection. SCM applies
/// branch, graph and detail results against this captured refresh identity.
pub(crate) struct RepositoryContext {
    repo: RepoKey,
    host: SharedHost,
    epoch: u64,
}

impl RepositoryContext {
    pub(crate) fn into_parts(self) -> (RepoKey, SharedHost, u64) {
        (self.repo, self.host, self.epoch)
    }
}

/// The four sections of the file list, in the order they are rendered.
///
/// `Merge` only appears while a merge is unresolved, which is why the panel
/// asks for it by variant rather than always drawing a header.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ScmGroup {
    Merge,
    Staged,
    Changes,
    Untracked,
}

impl ScmGroup {
    pub(crate) const ORDER: [ScmGroup; 4] = [
        ScmGroup::Merge,
        ScmGroup::Staged,
        ScmGroup::Changes,
        ScmGroup::Untracked,
    ];
}

#[derive(Default)]
pub(crate) struct ScmPanelState {
    /// Which repository the panel is showing. Follows the active pane unless
    /// `repo_override` says otherwise.
    repo: Option<RepoKey>,
    /// Set when the user picks a repository from the multi-repo dropdown, and
    /// cleared whenever the active tab changes — an explicit choice should
    /// outlive a pane switch inside one tab, not a jump to somewhere else.
    repo_override: Option<RepoKey>,
    /// The tab the override was made on, so the jump away can be noticed.
    override_tab: Option<usize>,
    /// Local branch names per repository, with the epoch they were read at.
    /// Anything that could have moved a ref bumps the epoch, so the list in
    /// the switcher is never older than the last operation.
    branches: HashMap<RepoKey, (u64, Vec<String>)>,
    branches_loading: HashMap<RepoKey, u64>,
    branch_generation: u64,
    /// The inline "name your branch" input, present only while it is open.
    new_branch: Option<Entity<InputState>>,
    /// The inline "switch to which branch" input, present only while it is
    /// open. Never open at the same time as `new_branch` — opening either one
    /// closes the other, since the panel has room for one input row.
    checkout_branch: Option<Entity<InputState>>,
    /// Unsent commit messages, one per working tree.
    drafts: HashMap<RepoKey, String>,
    /// The commit box. `None` until the panel has been rendered once: an
    /// `TextareaState` needs a real window to be created in, and this struct is
    /// built by `Default` alongside the rest of `LyrelApp`.
    commit_input: Option<Entity<TextareaState>>,
    /// Which repository's draft the box is currently holding. A change here
    /// is what moves one draft out and the next one in.
    commit_repo: Option<RepoKey>,
    /// A commit that has been dispatched: the repository, what HEAD was
    /// before it, and the message it carried. Held until HEAD moves, so a
    /// commit a hook rejects leaves the message in the box.
    committing: Option<(RepoKey, HeadState, String)>,
    /// Whether the next commit rewrites HEAD. Armed from the commit dropdown
    /// rather than a checkbox row — 260px does not have a row to spare.
    amend: bool,
    /// Groups the user folded shut, and the ones whose fold state they have
    /// set at all. Both are needed: a group nobody has touched follows the
    /// default for its size (a thousand untracked files start folded), and
    /// opening one by hand has to outlast the next file landing in it.
    pub(crate) collapsed: HashSet<ScmGroup>,
    pub(crate) toggled: HashSet<ScmGroup>,
    /// Directories folded shut in the tree view, per group — the same
    /// directory can be open under Changes and shut under Staged. Keyed by
    /// the full repo-relative path of the (possibly compacted) row.
    pub(crate) folded_dirs: HashSet<(ScmGroup, String)>,
    /// The filter over the changed files. Created on first render like
    /// `commit_input`, with the subscription that turns typing into a repaint.
    pub(crate) filter: Option<Entity<InputState>>,
    pub(crate) filter_sub: Option<gpui_kit::Subscription>,
    /// Working directory → the repository root containing it, or `None` when
    /// there is none, with when the answer was given. The root is what every
    /// write runs from and what every cache is keyed by, so it is resolved
    /// once per directory and reused.
    pub(crate) roots: HashMap<(HostId, PathBuf), (std::time::Instant, Option<PathBuf>)>,
    pub(crate) root_lookups: HashSet<(HostId, PathBuf)>,
    /// When the panel last asked for a status that it did not get back.
    pub(crate) probe_attempt: HashMap<(HostId, PathBuf), std::time::Instant>,
    /// The status the last frame drew, as (cache key, `Arc` identity). The
    /// watcher compares against it so a global write that changed nothing does
    /// not ask for another frame.
    pub(crate) seen: Option<((HostId, PathBuf), usize)>,
    pub(crate) watch: Option<gpui_kit::Subscription>,
    /// The cheap per-tab git status the panel last reacted to. A change in it
    /// means a command touched the repository and the expensive status is due
    /// another look.
    pub(crate) last_tab_status: Option<crate::terminal::git_status::GitStatus>,
    graph: GraphState,
    /// When set, the panel body is replaced by a single commit's detail view
    /// instead of the working tree.
    detail: Option<CommitDetailView>,
    detail_generation: u64,
    pub(crate) scroll: gpui_kit::ScrollHandle,
}

impl ScmPanelState {
    pub(crate) fn new(graph_expanded: bool) -> Self {
        Self {
            graph: GraphState {
                expanded: graph_expanded,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub(crate) fn pane_repo(&self) -> Option<&RepoKey> {
        self.repo.as_ref()
    }

    pub(crate) fn follow_pane(&mut self, repo: RepoKey) {
        self.repo = Some(repo);
    }

    pub(crate) fn enter_tab(&mut self, tab: usize) {
        if self.override_tab != Some(tab) {
            self.repo_override = None;
            self.override_tab = None;
        }
    }

    pub(crate) fn select_repository(&mut self, repo: RepoKey, tab: usize) {
        self.repo_override = Some(repo);
        self.override_tab = Some(tab);
    }

    pub(crate) fn repository_context(&mut self, cx: &App) -> Option<RepositoryContext> {
        if self
            .repo_override
            .as_ref()
            .is_some_and(|repo| HostRegistry::lookup(cx, repo.host).is_none())
        {
            self.repo_override = None;
            self.override_tab = None;
        }
        self.active_repo()?.context(cx)
    }

    /// Choices are resolved working-tree roots, never project-home guesses.
    pub(crate) fn repository_choices(&self) -> Vec<RepoKey> {
        let mut out = Vec::new();
        for ((host, _), (_, root)) in &self.roots {
            let Some(root) = root else { continue };
            let key = RepoKey {
                host: *host,
                root: root.clone(),
            };
            if !out.contains(&key) {
                out.push(key);
            }
        }
        out.sort_by(|a, b| a.root.cmp(&b.root));
        out
    }

    pub(crate) fn amend(&self) -> bool {
        self.amend
    }

    pub(crate) fn toggle_amend(&mut self) {
        self.amend = !self.amend;
    }

    pub(crate) fn message(&self, repo: &RepoKey, cx: &App) -> String {
        match (&self.commit_input, &self.commit_repo) {
            (Some(input), Some(showing)) if showing == repo => input.read(cx).value().to_string(),
            _ => self.draft(repo).to_string(),
        }
    }

    /// Share commit validation with the visible button without changing any
    /// operation state until the window admits the actual dispatch.
    pub(crate) fn prepare_commit(
        &self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        amend: bool,
        cx: &App,
    ) -> Result<lyrel_core::core::git::ops::GitOp, L10nKey> {
        let message = self.message(repo, cx);
        let plan = super::panel::commit_plan(status, amend, &message);
        if !plan.enabled {
            return Err(plan.reason);
        }
        Ok(lyrel_core::core::git::ops::GitOp::Commit {
            message,
            amend,
            signoff: false,
            no_verify: false,
            all: super::panel::commit_stages_everything(status, amend),
        })
    }

    /// The window adapter supplies geometry; SCM owns the retained input and
    /// exchanges drafts before changing its repository identity.
    pub(crate) fn commit_input(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        rows: std::ops::RangeInclusive<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TextareaState> {
        let input = self
            .commit_input
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(*rows.start(), *rows.end())
                        .placeholder(t(L10nKey::ScmCommitPlaceholder))
                })
            })
            .clone();
        let text = input.read(cx).value().to_string();
        if self.commit_repo.as_ref() != Some(repo) {
            if let Some(previous) = self.commit_repo.take() {
                self.drafts.insert(previous, text);
            }
            let next = self
                .drafts
                .get(repo)
                .cloned()
                .unwrap_or_else(|| status.prefilled_message.clone().unwrap_or_default());
            input.update(cx, |state, cx| state.set_value(next, window, cx));
            self.commit_repo = Some(repo.clone());
        } else if self.commit_landed(repo, &status.head, &text) {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        } else if self.drafts.get(repo).map(String::as_str) != Some(text.as_str()) {
            self.drafts.insert(repo.clone(), text);
        }
        input
    }

    /// Called only after confirmation and dispatch admission. Cancellation and
    /// validation rejection must neither disarm amend nor arm draft cleanup.
    pub(crate) fn commit_dispatched(&mut self, repo: RepoKey, head: HeadState, message: String) {
        self.amend = false;
        self.committing = Some((repo, head, message));
    }

    pub(crate) fn commit_failed(&mut self, host: HostId, root: &std::path::Path) {
        if self
            .committing
            .as_ref()
            .is_some_and(|(repo, _, _)| repo.host == host && repo.root == root)
        {
            self.committing = None;
        }
    }

    /// Dispatch is not success: only a changed HEAD with the same repository
    /// and still-unedited message permits the existing draft cleanup.
    fn commit_landed(&mut self, repo: &RepoKey, head: &HeadState, text: &str) -> bool {
        let Some((sent_repo, before, message)) = &self.committing else {
            return false;
        };
        if sent_repo != repo || before == head || message != text {
            return false;
        }
        self.committing = None;
        self.drafts.remove(repo);
        true
    }

    /// The repository the panel should act on: an explicit pick wins over
    /// whatever the active pane happens to be sitting in.
    pub(crate) fn active_repo(&self) -> Option<&RepoKey> {
        self.repo_override.as_ref().or(self.repo.as_ref())
    }

    /// Drop everything keyed to a host that left the registry. Without this,
    /// `roots` and friends grow one entry per directory ever visited on a
    /// link that no longer exists. Drafts survive on purpose: a reconnect
    /// brings the same repositories back, and unsent messages with them.
    pub(crate) fn forget_host(&mut self, host: HostId) {
        self.roots.retain(|(h, _), _| *h != host);
        self.root_lookups.retain(|(h, _)| *h != host);
        self.probe_attempt.retain(|(h, _), _| *h != host);
        self.branches.retain(|k, _| k.host != host);
        self.branches_loading.retain(|k, _| k.host != host);
        self.invalidate_history_host(host);
        if self.repo_override.as_ref().is_some_and(|k| k.host == host) {
            self.repo_override = None;
            self.override_tab = None;
        }
    }

    /// A probe just answered "no repository" for `root`: every directory that
    /// resolved to it has to re-ask. Left cached, the panel keeps mapping the
    /// cwd to a repository that is gone and draws its Loading state forever.
    pub(crate) fn forget_root(&mut self, host: HostId, root: &std::path::Path) {
        self.roots
            .retain(|(h, _), (_, held)| !(*h == host && held.as_deref() == Some(root)));
    }

    pub(crate) fn draft(&self, repo: &RepoKey) -> &str {
        self.drafts.get(repo).map(String::as_str).unwrap_or("")
    }

    /// Whether a group renders folded. `count` decides it only for a group the
    /// user has never touched.
    pub(crate) fn group_collapsed(&self, group: ScmGroup, count: usize) -> bool {
        if self.toggled.contains(&group) {
            self.collapsed.contains(&group)
        } else {
            crate::ui::scm::panel::starts_collapsed(group, count)
        }
    }

    /// Fold a tree directory shut, or open it again.
    pub(crate) fn toggle_dir(&mut self, group: ScmGroup, key: String) {
        let entry = (group, key);
        if !self.folded_dirs.remove(&entry) {
            self.folded_dirs.insert(entry);
        }
    }

    pub(crate) fn set_group_collapsed(&mut self, group: ScmGroup, collapsed: bool) {
        self.toggled.insert(group);
        if collapsed {
            self.collapsed.insert(group);
        } else {
            self.collapsed.remove(&group);
        }
    }
}

#[derive(Default)]
pub(crate) struct GraphState {
    /// Mirrors `Config::scm_graph_expanded`, which starts `false`: the history
    /// section unfurling on first open would make the panel look like a mess
    /// nobody asked for.
    pub(crate) expanded: bool,
    /// How many commits have been asked for so far. Paging grows this and
    /// re-runs the query rather than using `--skip`, which is O(skip) to walk
    /// and shifts under you when a ref moves between pages.
    pub(crate) requested: usize,
    pub(crate) loading: bool,
    /// The exact in-flight query and its generation; old callbacks cannot
    /// release a newer load or replace another repository/scope's page.
    loading_key: Option<(RepoKey, u64, GraphScope, u64)>,
    generation: u64,
    /// The page the graph is currently drawn from, and which repository and
    /// query produced it. `Arc` because paint reads it while the next page is
    /// being laid out on a worker, and the key so a repository switch shows an
    /// empty graph rather than the previous repository's history.
    pub(crate) page: Option<Arc<CommitPage>>,
    pub(crate) page_key: Option<(RepoKey, u64, GraphScope)>,
    /// The key of a load that came back empty-handed. Without it a failing
    /// `git log` — a scope pinned to a since-deleted branch, a vanished
    /// repository — would be retried from every frame's render, one git
    /// process per notify, forever. The failure clears itself the moment the
    /// key changes: an epoch bump (any refresh), a new scope, a new repo.
    pub(crate) failed_key: Option<(RepoKey, u64, GraphScope)>,
    /// Filter box. Like `commit_input`, created on first render — and with the
    /// subscription that turns typing into a repaint. An `InputState` is its
    /// own entity; without this the box would take text the list never sees.
    pub(crate) search: Option<Entity<InputState>>,
    pub(crate) search_sub: Option<gpui_kit::Subscription>,
    /// Which commit indices the list shows, cached per (page identity, query).
    /// The filter case-folds every subject and author; re-running that over
    /// 5000 commits on every frame while the box is open is real work, and
    /// even the unfiltered identity list is 40KB of indices a frame.
    ///
    /// The page is held as a `Weak`, not as its address: a freed page's
    /// allocation gets reused by the next one, and a bare pointer then says
    /// "same page" for a shorter history — indices past its end, and a panic.
    /// While this `Weak` lives the allocation cannot be handed out again.
    pub(crate) filter_cache: Option<(Option<String>, Weak<CommitPage>, Arc<Vec<usize>>)>,
    /// An open "name a branch at this commit" input, and the rev it starts
    /// from. The panel's own naming row cannot serve this: it always creates
    /// at HEAD, and the whole point here is the commit under the cursor.
    pub(crate) naming: Option<(Entity<InputState>, String)>,
    /// Which refs the graph walks from. Three states rather than an
    /// `Option<branch>`: "this branch and its upstream", "one named branch",
    /// and "everything" are all reachable from the header's dropdown, and only
    /// the enum the data layer already takes can express all three.
    pub(crate) scope: GraphScope,
    /// The selected row, by full sha.
    pub(crate) selected: Option<String>,
    pub(crate) scroll: gpui_kit::ScrollHandle,
    /// Height of the history section, and whether its divider is being
    /// dragged. Shaped like `right_panel_width` / `right_panel_dragging` so
    /// the same drag code works on the other axis.
    pub(crate) height: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) dragging: std::rc::Rc<std::cell::Cell<bool>>,
}

/// The panel's second-level view: one commit's metadata and the files it
/// touched. A file-level diff is not shown here — that opens the full-screen
/// overlay, because 260px cannot render a diff and pretending otherwise would
/// mean inventing a third kind of container.
///
/// The two loaded halves are behind `Arc` because the panel clones this whole
/// struct once per frame — `render_panel_scm` cannot hand `render_commit_
/// detail` a borrow of `self.scm` and a `&mut self` at once — and a commit
/// that touched a thousand files would otherwise deep-copy a thousand paths
/// every time anything on the panel redrew.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CommitDetailView {
    pub(crate) repo: RepoKey,
    pub(crate) oid: String,
    /// A read is out. Set before it is dispatched, so the render that runs in
    /// between does not ask for a second one.
    pub(crate) loading: bool,
    /// Whether a read has ever come back. With `loading` it is what stops the
    /// view asking again forever after a commit git could not resolve: the
    /// pair says "nothing is in flight and nothing is coming".
    pub(crate) loaded: bool,
    /// `None` until a read lands, and still `None` afterwards for a commit
    /// that is not in this repository.
    pub(crate) commit: Option<Arc<Commit>>,
    pub(crate) files: Option<Arc<Vec<CommitFile>>>,
    /// The file read came back with nothing — git errored, or the link
    /// dropped mid-read. Distinct from "still loading", and from an empty
    /// list: "0 files changed" for a commit whose files could not be read
    /// would be a confident lie.
    pub(crate) files_failed: bool,
    /// A long body starts folded — a merge from a bot can run to fifty lines,
    /// and the file list is what the reader came for.
    pub(crate) body_expanded: bool,
}

impl CommitDetailView {
    /// A commit the panel is about to show. `seed` is the row the graph
    /// already has in hand, where the caller came from the graph: the page
    /// carries every field a detail view needs, so handing it over is what
    /// keeps the common path from running `git show` for a commit that is
    /// literally on screen.
    fn new(repo: RepoKey, oid: String, seed: Option<Commit>) -> CommitDetailView {
        CommitDetailView {
            repo,
            oid,
            loading: false,
            loaded: false,
            commit: seed.map(Arc::new),
            files: None,
            files_failed: false,
            body_expanded: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(root: &str) -> RepoKey {
        RepoKey {
            host: HostId::LOCAL,
            root: PathBuf::from(root),
        }
    }

    #[test]
    fn an_explicit_repo_pick_outranks_the_active_pane() {
        let mut state = ScmPanelState::default();
        assert!(state.active_repo().is_none());
        state.follow_pane(key("/a"));
        assert_eq!(state.active_repo(), Some(&key("/a")));
        state.select_repository(key("/b"), 1);
        assert_eq!(state.active_repo(), Some(&key("/b")));
        state.enter_tab(1);
        state.follow_pane(key("/c"));
        assert_eq!(state.active_repo(), Some(&key("/b")));
        state.enter_tab(2);
        assert_eq!(state.active_repo(), Some(&key("/c")));
    }

    #[test]
    fn drafts_are_keyed_by_repository_not_by_path_alone() {
        let local = key("/a");
        let remote = RepoKey {
            host: HostId::from_connection_key("ssh-direct:me@box:22"),
            root: local.root.clone(),
        };
        let mut state = ScmPanelState::default();
        state.drafts.insert(local.clone(), "local wip".into());
        state.drafts.insert(remote.clone(), "remote wip".into());
        state.select_repository(remote.clone(), 1);
        state.forget_host(remote.host);
        assert_eq!(state.draft(&local), "local wip");
        assert_eq!(state.draft(&remote), "remote wip");
        state.select_repository(remote.clone(), 1);
        assert_eq!(state.active_repo(), Some(&remote));
    }

    #[gpui_kit::test]
    fn an_offline_override_never_borrows_the_panes_host(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let local = key("/local");
            let remote = RepoKey {
                host: HostId::from_connection_key("ssh-direct:me@offline:22"),
                root: PathBuf::from("/remote"),
            };
            let mut state = ScmPanelState::default();
            state.follow_pane(local.clone());
            state.select_repository(remote.clone(), 1);
            assert!(remote.context(cx).is_none());
            let (repo, host, epoch) = state.repository_context(cx).unwrap().into_parts();
            assert_eq!(repo, local);
            assert_eq!(host.id(), HostId::LOCAL);
            assert_eq!(epoch, 0);
            assert_eq!(state.active_repo(), Some(&local));
        });
    }
}
