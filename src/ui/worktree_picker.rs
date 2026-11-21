//! The shared worktree picker: one searchable list of every checkout of the
//! Context Pane's repository, opened from the Changes header's worktree
//! entry or the status bar's Git indicator (#351).
//!
//! Picking a checkout navigates: a pane of this window already sitting in
//! the target — its cwd somewhere underneath the checkout root counts — is
//! focused; otherwise exactly one terminal tab opens at the checkout root on
//! the owning Host. Nothing is ever sent to a running process: no `cd`, no
//! branch checkout, no injection. Creating a worktree hands the picker's
//! captured Host and directory to the existing creation prompt, so setup,
//! cancellation and per-worktree commit drafts behave exactly the way they
//! do from every other entrance.
//!
//! The pick a row names is a [`WorktreePick`] — the canonical checkout on its
//! owning Host. Removal (#352) consumes the same value through
//! [`LyrelApp::revalidate_worktree_pick`], which re-reads the checkout's
//! current facts on that Host before anything destructive is offered.

use std::path::{Path, PathBuf};

use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectUp};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, Focusable as _, IntoElement, MouseButton, MouseDownEvent,
    SharedString, Subscription, Task, Window, div, prelude::*, px, rems,
};

use crate::ui::app::LyrelApp;
use crate::ui::notification_history::NotificationWindowExt as _;
use crate::ui::right_panel::{META, info_chip};
use lyrel_ui_base::host_ops::SharedHost;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

/// The card's fixed width — two-line rows need more room than the 260px
/// panel the picker opens over.
const CARD_W: f32 = 480.;
/// One row carries two lines: the directory name, then branch and
/// distinguishing path. Every row is the same height, which the list
/// component requires.
const ROW_H: f32 = 44.;
const ROW_PAD: f32 = 10.;
const LIST_PAD: f32 = 8.;
/// The list stops growing at five rows; past that it scrolls.
const LIST_MAX_ROWS: usize = 5;
const TEXT: f32 = 13. / 16.;

/// A checkout the picker lists, bound to the Host that owns the repository.
/// Identity is the Host plus the canonical checkout path — never the
/// basename or the branch, which two checkouts can share (#351).
#[derive(Clone)]
pub(crate) struct WorktreePick {
    pub(crate) host: SharedHost,
    pub(crate) checkout: lyrel_core::core::worktree::Listed,
}

/// One checkout's current facts, re-read on its owning Host. The picker
/// itself consumes only the navigation half of a pick; removal (#352)
/// consumes this revalidation before offering to delete anything.
pub(crate) struct WorktreePickState {
    pub(crate) pick: WorktreePick,
    /// The checkout is still registered in `git worktree list`.
    pub(crate) present: bool,
    /// Uncommitted changes, untracked files included.
    pub(crate) dirty: bool,
    /// A pane of this window is sitting in the checkout.
    pub(crate) occupied: bool,
}

/// The picker, while open. One instance serves both entrances: they name the
/// same choice, so opening one closes the other.
pub(crate) struct WorktreePicker {
    /// The Host the list was read on — also where a new tab opens.
    host: SharedHost,
    /// The Context Pane's directory when the picker opened. The read runs
    /// from here, and the checkout containing it reads as the active row.
    cwd: PathBuf,
    query: Entity<InputState>,
    list: Entity<ListState<WorktreeDelegate>>,
    _sub: Subscription,
    _query_sub: Subscription,
}

/// The list's rows: every checkout of the repository, filtered by the query.
struct WorktreeDelegate {
    host: SharedHost,
    query: String,
    /// What `git worktree list` answered, main checkout first (git's own
    /// order, kept — the main checkout is the one removal never touches).
    worktrees: Vec<lyrel_core::core::worktree::Listed>,
    /// Indices into `worktrees` the query keeps visible.
    matching: Vec<usize>,
    /// The checkout containing the Context Pane's directory: what Return
    /// re-affirms, and the row the picker opens on.
    active: Option<PathBuf>,
    loading: bool,
    failed: Option<String>,
    selected: Option<IndexPath>,
}

impl WorktreeDelegate {
    fn new(host: SharedHost) -> Self {
        Self {
            host,
            query: String::new(),
            worktrees: Vec::new(),
            matching: Vec::new(),
            active: None,
            loading: true,
            failed: None,
            selected: None,
        }
    }

    /// The read landed (or failed). Returns the row the selection should
    /// open on: the active checkout when it is listed, otherwise the first.
    fn land(
        &mut self,
        worktrees: Vec<lyrel_core::core::worktree::Listed>,
        active: Option<PathBuf>,
    ) -> Option<IndexPath> {
        self.loading = false;
        self.failed = None;
        self.active = active;
        self.worktrees = worktrees;
        self.refilter();
        let first = self
            .active
            .as_ref()
            .and_then(|active| {
                self.matching
                    .iter()
                    .position(|&ix| self.worktrees[ix].path == *active)
            })
            .or_else(|| (!self.matching.is_empty()).then_some(0));
        first.map(|row| IndexPath {
            section: 0,
            row,
            ..Default::default()
        })
    }

    fn fail(&mut self, error: String) {
        self.loading = false;
        self.failed = Some(error);
        self.worktrees.clear();
        self.matching.clear();
    }

    fn refilter(&mut self) {
        self.matching = self
            .worktrees
            .iter()
            .enumerate()
            .filter(|(_, wt)| self.matches(wt))
            .map(|(ix, _)| ix)
            .collect();
    }

    /// Searchable by directory, branch and path — the three things a reader
    /// can know a checkout by. A detached HEAD matches nothing under branch;
    /// the detached label is a rendering of absence, not a name.
    fn matches(&self, wt: &lyrel_core::core::worktree::Listed) -> bool {
        let Some(query) = crate::ui::scm::tree::normalize_query(&self.query) else {
            return true;
        };
        let name = checkout_name(wt);
        let branch = wt.branch.as_deref().unwrap_or("");
        let path = wt.path.display().to_string();
        name.to_lowercase().contains(&query)
            || (!branch.is_empty() && branch.to_lowercase().contains(&query))
            || path.to_lowercase().contains(&query)
    }

    fn row(&self, row: usize) -> Option<&lyrel_core::core::worktree::Listed> {
        self.matching.get(row).map(|&ix| &self.worktrees[ix])
    }

    /// The pick a row names — the canonical checkout on its owning Host.
    fn pick_at(&self, ix: IndexPath) -> Option<WorktreePick> {
        let checkout = self.row(ix.row)?.clone();
        Some(WorktreePick {
            host: self.host.clone(),
            checkout,
        })
    }
}

impl ListDelegate for WorktreeDelegate {
    type Item = WorktreeRow;

    fn perform_search(
        &mut self,
        query: &str,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        self.refilter();
        Task::ready(())
    }

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.matching.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let wt = self.row(ix.row)?;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let name = checkout_name(wt);
        let path_text = wt.path.display().to_string();
        let branch = wt
            .branch
            .clone()
            .unwrap_or_else(|| t(L10nKey::ScmDetached).to_string());
        let second = match wt.path.parent().map(|p| p.display().to_string()) {
            Some(parent) if !parent.is_empty() => format!("{branch} · {parent}"),
            _ => branch.clone(),
        };
        let active = self.active.as_ref() == Some(&wt.path);
        let label: SharedString = format!("{name}, {branch}, {path_text}").into();

        let mut first = h_flex().flex_1().min_w_0().items_center().gap(px(6.));
        first = first.child(
            div()
                .flex_1()
                .min_w(px(40.))
                .truncate()
                .text_size(rems(TEXT))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(name),
        );
        if active {
            first = first.child(info_chip(
                &t(L10nKey::WorktreePickerCurrent),
                muted.opacity(0.14),
                muted,
                &mono,
            ));
        }

        let child = h_flex()
            .flex_1()
            .min_w_0()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .gap(px(2.))
                    .child(first)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(second),
                    ),
            )
            .into_any_element();

        Some(WorktreeRow {
            id: ("worktree-row", ix.row as u64).into(),
            label,
            selected: self.selected == Some(ix),
            child,
        })
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let text = match (&self.failed, self.loading) {
            (Some(error), _) => t_fmt(L10nKey::WorktreePickerFailed, &[("error", error)]),
            (None, true) => t(L10nKey::PanelLoading).to_string(),
            (None, false) => t(L10nKey::WorktreePickerEmpty).to_string(),
        };
        h_flex()
            .id("worktree-picker-empty")
            .w_full()
            .h(px(ROW_H))
            .px(px(ROW_PAD + LIST_PAD))
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(META))
                    .text_color(cx.theme().muted_foreground)
                    .child(text),
            )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        // After a search the list re-picks from the rows it drew last frame;
        // when that frame drew none, Return would do nothing on a list with
        // rows in it. Put the first row back once the list is done, the same
        // re-arm SearchDelegate performs.
        if ix.is_none() && !self.matching.is_empty() {
            let first = IndexPath {
                section: 0,
                row: 0,
                ..Default::default()
            };
            cx.defer_in(window, move |state, window, cx| {
                if state.selected_index().is_none() {
                    state.set_selected_index(Some(first), window, cx);
                }
            });
        }
        let moved = self.selected != ix;
        self.selected = ix;
        if moved && let Some(ix) = ix {
            cx.emit(ListEvent::Select(ix));
        }
        cx.notify();
    }
}

/// The directory name a checkout answers to — the last path component, with
/// the full path only as the fallback for a checkout at a filesystem root,
/// where there is no name to take.
fn checkout_name(wt: &lyrel_core::core::worktree::Listed) -> String {
    wt.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| wt.path.display().to_string())
}

/// One checkout row: the same neutral selected fill the search rows use, so
/// the keyboard row reads as where Return lands, not as an action.
#[derive(IntoElement)]
struct WorktreeRow {
    id: gpui_kit::ElementId,
    label: SharedString,
    selected: bool,
    child: AnyElement,
}

impl Selectable for WorktreeRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl gpui_kit::RenderOnce for WorktreeRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let sf = cx.global::<lyrel_ui_base::presets::Surfaces>().popover;
        let (hover, picked) = (gpui_kit::rgb(sf.hover), gpui_kit::rgb(sf.selected));
        h_flex()
            .id(self.id)
            .role(gpui_kit::Role::ListBoxOption)
            .aria_label(self.label)
            .aria_selected(self.selected)
            .items_center()
            .flex_none()
            .h(px(ROW_H))
            .mx(px(LIST_PAD))
            .px(px(ROW_PAD))
            .rounded(lyrel_ui_base::rounding::ROW_RADIUS)
            .overflow_hidden()
            .cursor_pointer()
            .text_color(cx.theme().foreground)
            .when(self.selected, |r| r.bg(picked))
            .when(!self.selected, |r| r.hover(move |s| s.bg(hover)))
            .child(self.child)
    }
}

impl LyrelApp {
    /// The shared entrance: open the picker on the Context Pane's
    /// repository. Both Git surfaces — the Changes header's worktree entry
    /// and the status bar's Git indicator — call this, so they cannot
    /// disagree about the choices or the active checkout. Opening while one
    /// is up closes it: a second entrance is a change of mind, not a second
    /// question.
    pub(crate) fn open_worktree_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktree_picker.is_some() {
            self.cancel_worktree_picker(window, cx);
            return;
        }
        let Some((host, cwd)) = self.scm_pane_target(window, cx) else {
            window.notify_app(t(L10nKey::PanelNoWorkingDirectory), cx);
            return;
        };
        let query = cx.new(|cx| InputState::new(window, cx));
        let delegate = WorktreeDelegate::new(host.clone());
        let list = cx.new(|cx| ListState::new(delegate, window, cx));
        let _sub = cx.subscribe_in(&list, window, Self::on_worktree_list_event);
        let _query_sub = cx.subscribe_in(
            &query,
            window,
            |this, input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.worktree_picker_query_changed(input, window, cx);
                }
            },
        );
        self.worktree_picker = Some(WorktreePicker {
            host: host.clone(),
            cwd: cwd.clone(),
            query,
            list,
            _sub,
            _query_sub,
        });
        if let Some(picker) = self.worktree_picker.as_ref() {
            picker.query.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();

        let read_host_id = host.id();
        let read_cwd = cwd.clone();
        lyrel_ui_base::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| -> Result<(Vec<lyrel_core::core::worktree::Listed>, Option<PathBuf>), String> {
                let worktrees = crate::core::worktree::list(h, &read_cwd)?;
                // The active checkout is the one the Context Pane sits in,
                // including from a subdirectory — the same prefix rule pane
                // reuse uses, asked on the Host that owns the paths.
                let cwd = h.canonicalize(&read_cwd).unwrap_or(read_cwd.clone());
                let active = worktrees
                    .iter()
                    .find(|wt| cwd.starts_with(&wt.path))
                    .map(|wt| wt.path.clone());
                Ok((worktrees, active))
            },
            move |this, result, window, cx| {
                this.land_worktree_list(read_host_id, &cwd, result, window, cx)
            },
        );
    }

    fn worktree_picker_query_changed(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.worktree_picker.as_ref() else {
            return;
        };
        let query = input.read(cx).value().to_string();
        picker
            .list
            .update(cx, |list, cx| list.set_query(&query, window, cx));
        cx.notify();
    }

    /// Forward the row's list chords to the list while the query input owns
    /// the keyboard, the same composed-input pattern SearchView uses.
    fn dispatch_worktree_list_action(
        &mut self,
        action: &dyn gpui_kit::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.worktree_picker.as_ref() else {
            return;
        };
        picker
            .list
            .read(cx)
            .focus_handle(cx)
            .dispatch_action(action, window, cx);
    }

    fn on_worktree_list_event(
        &mut self,
        list: &Entity<ListState<WorktreeDelegate>>,
        ev: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ListEvent::Confirm(ix) => {
                let pick = list.read(cx).delegate().pick_at(*ix);
                if let Some(pick) = pick {
                    self.confirm_worktree_pick(pick, window, cx);
                }
            }
            ListEvent::Cancel => self.cancel_worktree_picker(window, cx),
            ListEvent::Select(_) => {}
        }
    }

    /// The `git worktree list` read landed. It only applies while the picker
    /// it was started for is still the open one — a closed picker, or one
    /// opened for another Host or directory since, drops the answer.
    fn land_worktree_list(
        &mut self,
        host_id: lyrel_ui_base::host_ops::HostId,
        cwd: &Path,
        result: Result<(Vec<lyrel_core::core::worktree::Listed>, Option<PathBuf>), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.worktree_picker.as_ref() else {
            return;
        };
        if picker.host.id() != host_id || picker.cwd.as_path() != cwd {
            return;
        }
        let list = picker.list.clone();
        let first = match result {
            Ok((worktrees, active)) => {
                list.update(cx, |list, _| list.delegate_mut().land(worktrees, active))
            }
            Err(error) => list.update(cx, |list, _| {
                list.delegate_mut().fail(error);
                None
            }),
        };
        list.update(cx, |list, cx| {
            if let Some(first) = first {
                list.set_selected_index(Some(first), window, cx);
                list.scroll_to_selected_item(window, cx);
            }
        });
        cx.notify();
    }

    /// Navigation: focus a pane of this window already sitting in the picked
    /// checkout, or open exactly one terminal tab at its root on the owning
    /// Host. The source pane is never touched — no `cd`, no process moved,
    /// no branch checked out under it.
    fn confirm_worktree_pick(
        &mut self,
        pick: WorktreePick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.worktree_picker = None;
        let workspace = self.workspace;
        let host_id = pick.host.id();
        let target = pick.checkout.path.clone();
        let candidates: Vec<(usize, gpui_kit::EntityId, PathBuf)> = self
            .tabs
            .iter()
            .enumerate()
            .flat_map(|(tab, t)| t.pane.terminals().into_iter().map(move |leaf| (tab, leaf)))
            .filter_map(|(tab, leaf)| {
                let view = leaf.read(cx);
                if view.host_id() != host_id {
                    return None;
                }
                let cwd = view.effective_host_cwd()?;
                Some((tab, leaf.entity_id(), cwd))
            })
            .collect();
        let host = pick.host.clone();
        let match_root = target.clone();
        lyrel_ui_base::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| -> Option<(usize, gpui_kit::EntityId)> {
                candidates.iter().find_map(|(tab, leaf, cwd)| {
                    h.canonicalize(cwd)
                        .is_ok_and(|c| c.starts_with(&match_root))
                        .then_some((*tab, *leaf))
                })
            },
            move |this, matched, window, cx| {
                if this.workspace != workspace {
                    return;
                }
                match matched {
                    Some((tab_index, leaf_id)) => {
                        this.activate(tab_index, window, cx);
                        let handle = this.tabs.get(tab_index).and_then(|tab| {
                            tab.pane
                                .terminals()
                                .into_iter()
                                .find(|leaf| leaf.entity_id() == leaf_id)
                                .map(|leaf| leaf.read(cx).focus_handle(cx))
                        });
                        if let Some(handle) = handle {
                            window.focus(&handle, cx);
                        }
                    }
                    None => {
                        this.open_terminal_at_checkout(target.clone(), window, cx);
                    }
                }
                cx.notify();
            },
        );
    }

    /// One terminal tab at `path`, the way `open_worktree_tab` does it —
    /// same insertion, same selection, same focus — minus the setup script
    /// and agent, which belong to creation, not navigation.
    fn open_terminal_at_checkout(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = Self::new_terminal(
            crate::ui::app::PendingTerminalRequest::shell(
                self.window_workspace(cx),
                Some(self.workspace),
                self.font_size,
                Some(path),
                None,
                None,
            ),
            window,
            cx,
        );
        self.remember_active_pane(window, cx);
        self.maximized = None;
        let insert_at = self.new_tab_insert_at(cx);
        let tab = crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(view));
        self.tabs.insert(insert_at, tab);
        self.tabs.select(insert_at);
        self.focus_active(window, cx);
        self.save_session(cx);
    }

    /// The picker's Create action: hand the captured Host and directory to
    /// the existing creation flow, prompt and cancellation included. The
    /// picker closes — the prompt is the dialog now.
    fn worktree_picker_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.worktree_picker.take() else {
            return;
        };
        cx.notify();
        self.new_worktree_at(picker.host, picker.cwd, window, cx);
    }

    /// Close the picker and put the keyboard back where the workspace was.
    pub(crate) fn cancel_worktree_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktree_picker.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Re-read a picked checkout's facts on its owning Host: still listed,
    /// dirty, and whether a pane of this window is sitting in it. Removal
    /// (#352) calls this before offering to delete; until then the picker
    /// itself uses it to keep a row's facts honest.
    pub(crate) fn revalidate_worktree_pick(
        &mut self,
        pick: WorktreePick,
        window: &mut Window,
        cx: &mut Context<Self>,
        land: impl FnOnce(&mut Self, WorktreePickState, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let host_id = pick.host.id();
        let target = pick.checkout.path.clone();
        let open: Vec<PathBuf> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .filter_map(|leaf| {
                let view = leaf.read(cx);
                (view.host_id() == host_id).then(|| view.effective_host_cwd())?
            })
            .collect();
        let host = pick.host.clone();
        lyrel_ui_base::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let listed = crate::core::worktree::list(h, &target).unwrap_or_default();
                let present = listed.iter().any(|wt| wt.path == target);
                // `status --porcelain` is the dirty answer the removal path
                // already trusts; a read failure blocks, never proceeds.
                let dirty = present
                    && !h
                        .git(&target, &["status", "--porcelain"])
                        .map(|out| out.stdout_trimmed().is_empty())
                        .unwrap_or(true);
                let occupied = crate::core::worktree::occupied(h, &target, &open);
                WorktreePickState {
                    pick: pick.clone(),
                    present,
                    dirty,
                    occupied,
                }
            },
            move |this, state, window, cx| land(this, state, window, cx),
        );
    }

    pub(crate) fn render_worktree_picker_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.worktree_picker.as_ref()?;
        let title = t(L10nKey::WorktreePickerTitle);
        let muted = cx.theme().muted_foreground;
        picker.query.update(cx, |input, cx| {
            input.set_placeholder(t(L10nKey::WorktreePickerPlaceholder), window, cx);
        });
        let rungs = crate::ui::dialog::popover_rungs(cx);
        let card = crate::ui::dialog::card("worktree-picker-card", title, CARD_W, cx)
            .child(crate::ui::dialog::header(title, cx))
            .child(
                h_flex()
                    .id("worktree-picker-search")
                    .key_context("List")
                    .flex_none()
                    // Keep the framework's list navigation without moving
                    // focus out of the input. These handlers belong only to
                    // this row: the list's propagated Cancel must not loop
                    // back into them.
                    .on_action(cx.listener(|this, action: &SelectUp, window, cx| {
                        this.dispatch_worktree_list_action(action, window, cx);
                    }))
                    .on_action(cx.listener(|this, action: &SelectDown, window, cx| {
                        this.dispatch_worktree_list_action(action, window, cx);
                    }))
                    .on_action(cx.listener(|this, action: &Confirm, window, cx| {
                        this.dispatch_worktree_list_action(action, window, cx);
                    }))
                    .on_action(cx.listener(|this, action: &Cancel, window, cx| {
                        this.dispatch_worktree_list_action(action, window, cx);
                    }))
                    .px(px(crate::ui::dialog::INSET))
                    // `dialog::body`'s own field gap (14): one step under the
                    // header, a breath above the list.
                    .pt(px(14.))
                    .child(
                        crate::ui::dialog::field(
                            Input::new(&picker.query)
                                .prefix(Icon::new(IconName::Search).text_color(muted))
                                .cleanable(true),
                            cx,
                        )
                        .h(px(crate::ui::dialog::FIELD_H)),
                    ),
            )
            .child(div().px(px(LIST_PAD)).py(px(LIST_PAD)).child(
                List::new(&picker.list).max_h(px(ROW_H * LIST_MAX_ROWS as f32 + LIST_PAD * 2.)),
            ))
            .child(
                crate::ui::dialog::footer(cx)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(6.))
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(crate::ui::dialog::chord(
                                crate::ui::keymap::key_tokens("enter"),
                                cx,
                            ))
                            .child(t(L10nKey::WorktreePickerTitle)),
                    )
                    .child(crate::ui::dialog::button(
                        "worktree-picker-new",
                        t(L10nKey::CmdNewWorktreeTab),
                        crate::ui::dialog::Tone::Secondary,
                        true,
                        rungs,
                        cx,
                        cx.listener(|this, _, window, cx| this.worktree_picker_create(window, cx)),
                    )),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(lyrel_ui_base::presets::scrim_fill(cx))
                .on_key_down(
                    cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            this.cancel_worktree_picker(window, cx);
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        // A dismissing press is spent on the dismissal, the
                        // switcher's rule — it must not carry on down to the
                        // entrance beneath and reopen the picker.
                        cx.stop_propagation();
                        this.cancel_worktree_picker(window, cx);
                    }),
                )
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                .pt(px(crate::ui::switcher::CARD_TOP))
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window::harness;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use lyrel_ui_base::host_ops::HostId;

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    /// A real repository with one linked worktree: the main checkout at
    /// `main_root`, the linked one at `wt_root` on its own branch.
    fn repository_with_worktree() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, PathBuf) {
        let main = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        let main_root = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(main.path()).unwrap(),
        );
        let wt_root = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(wt.path()).unwrap(),
        );
        let git = |args: &[&str], cwd: &Path| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"], &main_root);
        std::fs::write(main_root.join("file.txt"), "base\n").unwrap();
        git(&["add", "file.txt"], &main_root);
        git(
            &[
                "-c",
                "user.name=lyrel",
                "-c",
                "user.email=lyrel@example.test",
                "commit",
                "--quiet",
                "-m",
                "base",
            ],
            &main_root,
        );
        git(
            &[
                "worktree",
                "add",
                "--quiet",
                wt_root.to_str().unwrap(),
                "-b",
                "wt-branch",
            ],
            &main_root,
        );
        (main, wt, main_root, wt_root)
    }

    /// Point a quiet test pane at `cwd` and register the cheap probe answer
    /// (branch + counts) the status bar and picker read from.
    fn seed_repo_pane(
        vcx: &mut VisualTestContext,
        pane: &Entity<lyrel_terminal::view::TerminalView>,
        cwd: &Path,
        branch: &str,
    ) {
        use crate::ui::scm::status_cache::{GitStatusCache, RepoSnapshot};
        let cwd = cwd.to_path_buf();
        vcx.update(|_, cx| {
            pane.update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()));
                view.terminal.seed_cwd(Some(cwd.clone()));
            });
            cx.default_global::<GitStatusCache>().finish_probe(
                HostId::LOCAL,
                &cwd,
                Some(RepoSnapshot {
                    root: cwd.clone(),
                    home: cwd.clone(),
                    branch: branch.into(),
                    counts: Some((0, 0)),
                }),
            );
        });
    }

    fn add_repo_tab(
        app: &Entity<LyrelApp>,
        vcx: &mut VisualTestContext,
        pane_id: u64,
    ) -> (
        Entity<lyrel_terminal::view::TerminalView>,
        crate::daemon::transport::Stream,
    ) {
        app.update_in(vcx, |app, window, cx| {
            use crate::ui::pane::{Pane, PaneSlot};
            use lyrel_terminal::view::quiet_test_pane;
            let (view, stream) = quiet_test_pane(pane_id, window, cx);
            LyrelApp::watch_terminal(&view, window, cx);
            app.tabs
                .push(crate::ui::tab::Tab::new(Pane::leaf(PaneSlot::Ready(
                    view.clone(),
                ))));
            cx.notify();
            (view, stream)
        })
    }

    fn row_texts(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|window, _| {
            gpui_kit::base::test_support::texts(window)
                .into_iter()
                .filter(|text| {
                    text.path().iter().any(|id| {
                        matches!(
                            id,
                            gpui_kit::ElementId::NamedInteger(name, _)
                                if name.as_ref() == "worktree-row"
                        )
                    })
                })
                .map(|text| text.text().to_string())
                .collect()
        })
    }

    fn type_in_picker(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, query: &str) {
        app.update_in(vcx, |app, window, cx| {
            let picker = app.worktree_picker.as_ref().expect("picker open");
            picker.query.update(cx, |input, cx| {
                input.set_value(query, window, cx);
            });
        });
    }

    #[gpui_kit::test]
    fn status_bar_entrance_lists_checkouts_and_marks_the_active_one(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_picker.is_some()),
            "the status bar Git indicator opens the picker"
        );
        let rows = row_texts(&mut vcx);
        assert_eq!(rows.len(), 2, "main + linked checkout are listed");
        vcx.update(|window, _| {
            assert!(window.find("worktree-picker-card").visible());
            assert!(
                gpui_kit::base::test_support::texts(window)
                    .into_iter()
                    .any(|text| text.text() == "current"),
                "the Context Pane's checkout is marked active"
            );
        });

        // Search by branch narrows to the linked checkout; the
        // distinguishing path finds the same row again.
        type_in_picker(&app, &mut vcx, "wt-branch");
        settle(&mut vcx);
        let by_branch = row_texts(&mut vcx);
        assert_eq!(by_branch.len(), 1);
        assert!(by_branch[0].contains("wt-branch"));
        type_in_picker(&app, &mut vcx, wt_root.parent().unwrap().to_str().unwrap());
        settle(&mut vcx);
        let by_path = row_texts(&mut vcx);
        assert_eq!(
            by_path.len(),
            1,
            "the distinguishing path identifies the checkout"
        );
        assert_eq!(by_path, by_branch);

        // Escape closes without navigating.
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, _| app.worktree_picker.is_none()));
    }

    #[gpui_kit::test]
    fn picking_a_checkout_without_a_matching_pane_opens_one_tab_at_its_root(
        cx: &mut TestAppContext,
    ) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        let tabs_before = app.read_with(&vcx, |app, _| app.tabs.len());
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        settle(&mut vcx);
        type_in_picker(
            &app,
            &mut vcx,
            wt_root.file_name().unwrap().to_str().unwrap(),
        );
        settle(&mut vcx);
        vcx.simulate_keystrokes("enter");
        settle(&mut vcx);

        assert!(
            app.read_with(&vcx, |app, _| app.worktree_picker.is_none()),
            "confirming navigates and closes the picker"
        );
        let (tabs_after, selected, spawn_cwd) = app.read_with(&vcx, |app, cx| {
            let index = app.tabs.active_index();
            let cwd = app.tabs[index]
                .pane
                .first_leaf()
                .and_then(|slot| match slot {
                    crate::ui::pane::PaneSlot::Connecting(pending) => {
                        pending.read(cx).spawn_cwd().map(Path::to_path_buf)
                    }
                    _ => None,
                });
            (app.tabs.len(), index, cwd)
        });
        assert_eq!(tabs_after, tabs_before + 1, "exactly one tab opened");
        assert_eq!(selected, tabs_after - 1, "the new tab is selected");
        assert_eq!(
            spawn_cwd.as_deref(),
            Some(wt_root.as_path()),
            "the tab opens at the picked checkout's root"
        );

        // The source terminal is untouched: same cwd, same branch — nothing
        // was sent to the running process.
        let (source_cwd, source_branch) = app.read_with(&vcx, |app, cx| {
            let leaf = app.tabs[0].pane.first_leaf().expect("source pane");
            let view = leaf.terminal().expect("ready").read(cx);
            (
                view.git_status_cwd().map(Path::to_path_buf),
                view.git_status(cx).map(|g| g.branch),
            )
        });
        assert_eq!(source_cwd.as_deref(), Some(main_root.as_path()));
        assert_eq!(source_branch.as_deref(), Some("main"));
    }

    #[gpui_kit::test]
    fn picking_a_checkout_reuses_the_pane_already_in_it(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (left, _s1) = add_repo_tab(&app, &mut vcx, 1);
        let (right, _s2) = add_repo_tab(&app, &mut vcx, 2);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &left, &main_root, "main");
        // The second pane sits in a *subdirectory* of the linked checkout.
        let subdir = wt_root.join("src");
        std::fs::create_dir(&subdir).unwrap();
        seed_repo_pane(&mut vcx, &right, &subdir, "wt-branch");

        let tabs_before = app.read_with(&vcx, |app, _| app.tabs.len());
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        settle(&mut vcx);
        type_in_picker(
            &app,
            &mut vcx,
            wt_root.file_name().unwrap().to_str().unwrap(),
        );
        settle(&mut vcx);
        vcx.simulate_keystrokes("enter");
        settle(&mut vcx);

        assert_eq!(
            app.read_with(&vcx, |app, _| app.tabs.len()),
            tabs_before,
            "reusing a pane must not open a tab"
        );
        assert_eq!(
            app.read_with(&vcx, |app, _| app.tabs.active_index()),
            1,
            "the tab holding the matching pane is active"
        );
        vcx.update(|window, cx| {
            assert!(
                right.read(cx).focus_handle(cx).is_focused(window),
                "the pane inside the checkout holds focus"
            );
        });
    }

    #[gpui_kit::test]
    fn create_from_the_picker_opens_the_existing_prompt_and_cancels_cleanly(
        cx: &mut TestAppContext,
    ) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, _wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("worktree-picker-new", cx));
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_picker.is_none()),
            "the picker hands over to the existing creation prompt"
        );
        vcx.update(|window, _| {
            assert!(
                window.find("worktree-prompt-card").visible(),
                "the existing worktree prompt opens on the repository"
            );
        });
        let tabs_before = app.read_with(&vcx, |app, _| app.tabs.len());
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert!(
                !window.find("worktree-prompt-card").visible(),
                "cancelling the prompt closes it"
            );
        });
        assert_eq!(
            app.read_with(&vcx, |app, _| app.tabs.len()),
            tabs_before,
            "cancelling creation must not open a tab"
        );
    }
}
