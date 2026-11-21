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
//! current facts on that Host before anything destructive is offered, and
//! again after the confirmation click: the checkout may have become dirty or
//! occupied while the dialog was open. Dirty removal needs its own explicit
//! confirmation and a durable recovery point before Git removes the checkout.

use std::path::{Path, PathBuf};

use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectUp};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable, Sizable as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, Focusable as _, IntoElement, MouseButton, MouseDownEvent,
    SharedString, Subscription, Task, TestSupportExt as _, Window, div, prelude::*, px, rems,
};

use crate::ui::app::LyrelApp;
use crate::ui::notification_history::NotificationWindowExt as _;
use crate::ui::right_panel::{META, info_chip};
use lyrel_core::core::worktree::{
    CheckoutInspection, CheckoutRemoval, inspect_checkout, remove_confirmed_checkout,
};
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

/// One checkout's current facts, re-read on its owning Host. Removal (#352)
/// consumes this before offering to delete, and again after the confirmation
/// click: any fact that cannot be read is a block, never a pass, and a fact
/// that changed while the dialog was open blocks with the fresh reason.
pub(crate) struct WorktreePickState {
    /// The pinned identity: the canonical checkout path on its Host. Two
    /// external directories can share a basename, so identity is never the
    /// name and never the branch.
    pub(crate) canonical: PathBuf,
    /// The facts shown by this confirmation, rechecked before deletion.
    inspection: Option<CheckoutInspection>,
    /// What blocks removal right now; `None` while every guard passes.
    pub(crate) block: Option<WorktreePickBlock>,
}

/// Why one checkout cannot be removed from the disk right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreePickBlock {
    /// The checkout no longer answers `git worktree list` on its Host.
    Gone,
    /// The main checkout is never removable.
    Main,
    /// The checkout changed after the confirmation was shown.
    Changed,
    /// A pane of some workspace is sitting in the checkout.
    Occupied,
    /// An unsaved document is open inside the checkout.
    Unsaved,
    /// A Host read failed; nothing destructive proceeds on a guess.
    Unverifiable,
}

/// The removal confirmation for one pick, shown inside the picker's card.
/// Pinned at activation — the Host, the checkout identity, the canonical
/// path — and revalidated before the dialog is offered and once more after
/// the confirmation click.
struct WorktreeRemoval {
    id: uuid::Uuid,
    pick: WorktreePick,
    /// What the open-time revalidation answered; `None` while it runs.
    state: Option<WorktreePickState>,
    /// True from confirmation until its result; ignore repeated activation.
    busy: bool,
    /// Once Git is asked to remove, dismissal cannot cancel that operation.
    removing: bool,
}

/// The picker, while open. One instance serves both entrances: they name the
/// same choice, so opening one closes the other.
pub(crate) struct WorktreePicker {
    /// The Host the list was read on — also where a new tab opens.
    host: SharedHost,
    /// The Context Pane's spawn route, distinct from the window's route for
    /// a local sidecar inside a remote Workspace.
    spawn_workspace: Option<lyrel_ui_base::route::PaneWorkspace>,
    /// The Context Pane's directory when the picker opened. The read runs
    /// from here, and the checkout containing it reads as the active row.
    cwd: PathBuf,
    query: Entity<InputState>,
    list: Entity<ListState<WorktreeDelegate>>,
    /// The removal confirmation, while it is showing. One dialog at a time:
    /// the card asks one question, and the question is removal now.
    removal: Option<WorktreeRemoval>,
    /// Keyboard owner while the removal dialog shows: the query input owns
    /// the list's chords, the removal card owns its own.
    removal_focus: gpui_kit::FocusHandle,
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
            .test_support()
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
        let spawn_workspace = self
            .context_pane(window, cx)
            .and_then(|pane| pane.read(cx).workspace().cloned());
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
            spawn_workspace,
            cwd: cwd.clone(),
            query,
            list,
            removal: None,
            removal_focus: cx.focus_handle(),
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
        let Some(picker) = self.worktree_picker.take() else {
            return;
        };
        let spawn_workspace = picker.spawn_workspace;
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
                        this.open_terminal_at_checkout(target, spawn_workspace, window, cx);
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
        spawn_workspace: Option<lyrel_ui_base::route::PaneWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = Self::new_terminal(
            crate::ui::app::PendingTerminalRequest::shell(
                spawn_workspace,
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

    /// The picker's removal action, named for what it does: remove the
    /// selected checkout from the disk. Opens the confirmation inside the
    /// picker's card; nothing is deleted here.
    fn worktree_picker_remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = self.worktree_picker.as_ref().and_then(|picker| {
            let ix = picker.list.read(cx).delegate().selected?;
            picker.list.read(cx).delegate().pick_at(ix)
        });
        let Some(pick) = pick else {
            return;
        };
        let focus = match self.worktree_picker.as_ref() {
            Some(picker) => picker.removal_focus.clone(),
            None => return,
        };
        if let Some(picker) = self.worktree_picker.as_mut() {
            picker.removal = Some(WorktreeRemoval {
                id: uuid::Uuid::new_v4(),
                pick: pick.clone(),
                state: None,
                busy: false,
                removing: false,
            });
        }
        window.focus(&focus, cx);
        cx.notify();
        self.revalidate_open_worktree_removal(pick, window, cx);
    }

    /// The removal dialog backs out to the list; the picker itself stays
    /// open, the way the creation prompt's cancellation leaves the app.
    fn cancel_worktree_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.worktree_picker.as_mut() else {
            return;
        };
        if picker
            .removal
            .as_ref()
            .is_some_and(|removal| removal.removing)
        {
            return;
        }
        if picker.removal.take().is_some() {
            picker.query.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        }
    }

    /// The removal dialog's confirmation. The facts are re-read on the
    /// owning Host first — the checkout may have become dirty or occupied
    /// while the dialog was open — and only a still-removable checkout runs
    /// the deletion, which itself re-verifies every fact on the Host.
    fn confirm_worktree_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, pick, approved, cwd)) = self.worktree_picker.as_mut().and_then(|picker| {
            let removal = picker.removal.as_mut()?;
            let state = removal.state.as_ref()?;
            if removal.busy || state.block.is_some() {
                return None;
            }
            let approved = state.inspection.clone()?;
            removal.busy = true;
            Some((
                removal.id,
                removal.pick.clone(),
                approved,
                picker.cwd.clone(),
            ))
        }) else {
            return;
        };
        cx.notify();
        let host = pick.host.clone();
        let checkout = pick.checkout.clone();
        self.revalidate_worktree_pick(pick, cwd, window, cx, move |this, mut state, window, cx| {
            let Some(removal) = this
                .worktree_picker
                .as_mut()
                .and_then(|picker| picker.removal.as_mut())
            else {
                return;
            };
            if removal.id != id || !removal.busy {
                return;
            }
            if state.block.is_none() && state.inspection.as_ref() != Some(&approved) {
                state.block = Some(WorktreePickBlock::Changed);
            }
            if state.block.is_some() {
                removal.state = Some(state);
                removal.busy = false;
                cx.notify();
                return;
            }
            removal.removing = true;
            cx.notify();
            lyrel_ui_base::host_ops::HostOps::run_in(
                host,
                window,
                cx,
                move |h| remove_confirmed_checkout(h, &checkout, &approved),
                move |this, result, window, cx| {
                    this.finish_worktree_removal(id, result, window, cx)
                },
            );
        });
    }

    /// The deletion answered: report the distinct facts — the checkout is
    /// gone, the recovery reference, and whether the branch was deleted,
    /// kept or absent — and close the picker, the way a confirmed
    /// navigation does.
    fn finish_worktree_removal(
        &mut self,
        id: uuid::Uuid,
        result: Result<CheckoutRemoval, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, branch) = match self
            .worktree_picker
            .as_ref()
            .and_then(|picker| picker.removal.as_ref())
            .filter(|removal| removal.id == id)
        {
            Some(removal) => (
                removal.pick.checkout.path.display().to_string(),
                removal.pick.checkout.branch.clone().unwrap_or_default(),
            ),
            None => return,
        };
        match result {
            Ok(removed) => {
                let notice = match removed.branch {
                    lyrel_core::core::worktree::BranchOutcome::Deleted => t_fmt(
                        L10nKey::WorktreePickerRemovedBranchDeleted,
                        &[
                            ("path", &path),
                            ("branch", &branch),
                            ("ref", &removed.recovery),
                        ],
                    ),
                    lyrel_core::core::worktree::BranchOutcome::Kept => t_fmt(
                        L10nKey::WorktreePickerRemovedBranchKept,
                        &[
                            ("path", &path),
                            ("branch", &branch),
                            ("ref", &removed.recovery),
                        ],
                    ),
                    lyrel_core::core::worktree::BranchOutcome::Detached => t_fmt(
                        L10nKey::WorktreePickerRemovedDetached,
                        &[("path", &path), ("ref", &removed.recovery)],
                    ),
                };
                self.worktree_picker = None;
                self.focus_active(window, cx);
                window.notify_app(notice, cx);
                cx.notify();
            }
            Err(e) => {
                if let Some(picker) = self.worktree_picker.as_mut() {
                    if let Some(removal) = picker.removal.as_mut() {
                        removal.busy = false;
                        removal.removing = false;
                    }
                }
                window.notify_app(
                    lyrel_ui_base::host_ops::failure(
                        t_fmt(L10nKey::WorktreePickerRemoveFailed, &[("error", &e)]),
                        &e,
                    ),
                    cx,
                );
                cx.notify();
            }
        }
    }

    /// A late answer belongs only to the specific confirmation that asked for it.
    fn revalidate_open_worktree_removal(
        &mut self,
        pick: WorktreePick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, cwd)) = self
            .worktree_picker
            .as_ref()
            .and_then(|picker| Some((picker.removal.as_ref()?.id, picker.cwd.clone())))
        else {
            return;
        };
        self.revalidate_worktree_pick(pick, cwd, window, cx, move |this, state, _window, cx| {
            let Some(removal) = this
                .worktree_picker
                .as_mut()
                .and_then(|picker| picker.removal.as_mut())
            else {
                return;
            };
            if removal.id == id && !removal.busy {
                removal.state = Some(state);
                cx.notify();
            }
        });
    }

    /// Close the picker and put the keyboard back where the workspace was.
    pub(crate) fn cancel_worktree_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .worktree_picker
            .as_ref()
            .and_then(|picker| picker.removal.as_ref())
            .is_some_and(|removal| removal.removing)
        {
            return;
        }
        if self.worktree_picker.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Re-read the linked checkout, live pane usage and unsaved files on its
    /// owning Host. Neither an unavailable status nor a stale answer grants consent.
    fn revalidate_worktree_pick(
        &mut self,
        pick: WorktreePick,
        repo_cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        land: impl FnOnce(&mut Self, WorktreePickState, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let host_id = pick.host.id();
        let target = pick.checkout.path.clone();
        let mut open: Vec<PathBuf> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .filter_map(|leaf| {
                let view = leaf.read(cx);
                (view.host_id() == host_id).then(|| view.effective_host_cwd())?
            })
            .collect();
        let mut unsaved = self.unsaved_worktree_documents(host_id, cx);
        if cx
            .try_global::<crate::ui::windows::WindowRegistry>()
            .is_some()
        {
            let current = cx.entity().entity_id();
            for (_, app) in crate::ui::windows::WindowRegistry::open_windows(cx) {
                if let Some(app) = app.upgrade().filter(|app| app.entity_id() != current) {
                    unsaved.extend(app.read(cx).unsaved_worktree_documents(host_id, cx));
                }
            }
        }
        let host = pick.host.clone();
        // In production, ask the daemon directly instead of trusting a possibly
        // stale mirror, which can miss panes created in other workspaces.
        let control = crate::ui::tree_sync::tree_control_for(cx, host_id);
        lyrel_ui_base::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let mut state = WorktreePickState {
                    canonical: target.clone(),
                    inspection: None,
                    block: None,
                };
                let Ok(canonical) = h.canonicalize(&target) else {
                    state.block = Some(WorktreePickBlock::Unverifiable);
                    return state;
                };
                state.canonical = canonical.clone();
                let Ok(listed) = crate::core::worktree::list(h, &repo_cwd) else {
                    state.block = Some(WorktreePickBlock::Unverifiable);
                    return state;
                };
                let found = listed.iter().find(|wt| {
                    h.canonicalize(&wt.path).ok().as_deref() == Some(canonical.as_path())
                });
                let Some(found) = found else {
                    state.block = Some(WorktreePickBlock::Gone);
                    return state;
                };
                if found.branch != pick.checkout.branch {
                    state.block = Some(WorktreePickBlock::Changed);
                    return state;
                }
                if found.main {
                    state.block = Some(WorktreePickBlock::Main);
                    return state;
                }
                use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
                let machine = match control {
                    crate::ui::tree_sync::TreeLink::Ready(client) => {
                        match client.call(ControlRequest::MachineGet) {
                            Ok(ReplyOk::MachineTree(machine)) => machine,
                            _ => {
                                state.block = Some(WorktreePickBlock::Unverifiable);
                                return state;
                            }
                        }
                    }
                    _ => {
                        state.block = Some(WorktreePickBlock::Unverifiable);
                        return state;
                    }
                };
                for pane in machine.panes.into_iter().filter(|pane| pane.live) {
                    let Some(cwd) = pane.cwd else {
                        state.block = Some(WorktreePickBlock::Unverifiable);
                        return state;
                    };
                    open.push(PathBuf::from(cwd));
                }
                match crate::core::worktree::occupancy(h, &canonical, &open) {
                    Ok(true) => {
                        state.block = Some(WorktreePickBlock::Occupied);
                        return state;
                    }
                    Ok(false) => {}
                    Err(_) => {
                        state.block = Some(WorktreePickBlock::Unverifiable);
                        return state;
                    }
                }
                for doc in &unsaved {
                    match h.canonicalize(doc) {
                        Ok(path) if path.starts_with(&canonical) => {
                            state.block = Some(WorktreePickBlock::Unsaved);
                            return state;
                        }
                        Ok(_) => {}
                        Err(_) => {
                            state.block = Some(WorktreePickBlock::Unverifiable);
                            return state;
                        }
                    }
                }
                match inspect_checkout(h, found) {
                    Ok(inspection) if inspection.canonical() == canonical => {
                        state.inspection = Some(inspection)
                    }
                    _ => state.block = Some(WorktreePickBlock::Unverifiable),
                }
                state
            },
            move |this, state, window, cx| land(this, state, window, cx),
        );
    }

    fn unsaved_worktree_documents(
        &self,
        host: lyrel_ui_base::host_ops::HostId,
        cx: &App,
    ) -> Vec<PathBuf> {
        self.editor
            .read(cx)
            .buffers()
            .iter()
            .filter(|file| {
                file.dirty && !file.path.as_os_str().is_empty() && file.host.id() == host
            })
            .map(|file| file.path.clone())
            .collect()
    }

    pub(crate) fn render_worktree_picker_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.worktree_picker.as_ref()?;
        let rungs = crate::ui::dialog::popover_rungs(cx);
        let card: AnyElement = if let Some(removal) = picker.removal.as_ref() {
            self.render_worktree_removal(picker, removal, window, cx)
                .into_any_element()
        } else {
            let title = t(L10nKey::WorktreePickerTitle);
            let muted = cx.theme().muted_foreground;
            picker.query.update(cx, |input, cx| {
                input.set_placeholder(t(L10nKey::WorktreePickerPlaceholder), window, cx);
            });
            crate::ui::dialog::card("worktree-picker-card", title, CARD_W, cx)
                .test_support()
                .child(crate::ui::dialog::header(title, cx))
                .child(
                    h_flex()
                        .id("worktree-picker-search")
                        .key_context("List")
                        .flex_none()
                        .w_full()
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
                            "worktree-picker-remove",
                            t(L10nKey::WorktreePickerRemove),
                            crate::ui::dialog::Tone::Secondary,
                            true,
                            rungs,
                            cx,
                            cx.listener(|this, _, window, cx| {
                                this.worktree_picker_remove_selected(window, cx)
                            }),
                        ))
                        .child(crate::ui::dialog::button(
                            "worktree-picker-new",
                            t(L10nKey::CmdNewWorktreeTab),
                            crate::ui::dialog::Tone::Secondary,
                            true,
                            rungs,
                            cx,
                            cx.listener(|this, _, window, cx| {
                                this.worktree_picker_create(window, cx)
                            }),
                        )),
                )
                .into_any_element()
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(lyrel_ui_base::presets::scrim_fill(cx))
                .on_key_down(
                    cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            // Escape backs out one level: the removal dialog
                            // first, then the picker itself.
                            if this
                                .worktree_picker
                                .as_ref()
                                .is_some_and(|picker| picker.removal.is_some())
                            {
                                this.cancel_worktree_removal(window, cx);
                            } else {
                                this.cancel_worktree_picker(window, cx);
                            }
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

    /// The removal confirmation: the checkout's full path, what may happen
    /// to its branch, and the revalidated answer to "may this be removed?"
    /// The destructive action is inert until every guard passes.
    fn render_worktree_removal(
        &self,
        picker: &WorktreePicker,
        removal: &WorktreeRemoval,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = t(L10nKey::WorktreePickerRemoveTitle);
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        // Identity is the canonical path once the revalidation has pinned it;
        // while it runs, the pick's own spelling is the best name available.
        let path = match &removal.state {
            Some(state) => state.canonical.display().to_string(),
            None => removal.pick.checkout.path.display().to_string(),
        };
        let branch_line = match removal.pick.checkout.branch.as_deref() {
            Some(branch) => t_fmt(L10nKey::WorktreePickerRemoveBranch, &[("branch", branch)]),
            None => t(L10nKey::WorktreePickerRemoveDetached).to_string(),
        };
        let dirty = removal
            .state
            .as_ref()
            .and_then(|state| state.inspection.as_ref())
            .is_some_and(CheckoutInspection::dirty);
        let (status, removable) = match (&removal.state, removal.busy) {
            (_, _) if removal.removing => (t(L10nKey::WorktreePickerRemoving).to_string(), false),
            (None, _) | (_, true) => (t(L10nKey::WorktreePickerRemoveChecking).to_string(), false),
            (Some(state), false) => match state.block {
                None if dirty => (t(L10nKey::WorktreePickerRemoveDirtyReady).to_string(), true),
                None => (
                    t(L10nKey::WorktreePickerRemoveReady).to_string(),
                    state.inspection.is_some(),
                ),
                Some(block) => (Self::worktree_pick_block_text(block), false),
            },
        };
        crate::ui::dialog::card("worktree-removal-card", title, CARD_W, cx)
            .test_support()
            .child(crate::ui::dialog::header(title, cx))
            .child(
                v_flex()
                    .gap(px(10.))
                    .px(px(crate::ui::dialog::INSET))
                    .pt(px(14.))
                    .pb(px(16.))
                    .child(
                        div()
                            .font_family(mono)
                            .text_size(rems(TEXT))
                            .id("worktree-removal-target")
                            .role(gpui_kit::Role::Label)
                            .aria_label(path.clone())
                            .test_support()
                            .child(path),
                    )
                    .child(
                        div()
                            .id("worktree-removal-branch")
                            .role(gpui_kit::Role::Label)
                            .aria_label(branch_line.clone())
                            .test_support()
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(branch_line),
                    )
                    .child(
                        div()
                            .id("worktree-removal-status")
                            .role(gpui_kit::Role::Label)
                            .aria_label(status.clone())
                            .test_support()
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(status),
                    ),
            )
            .child(
                crate::ui::dialog::footer(cx)
                    .child(
                        Button::new("worktree-removal-cancel")
                            .small()
                            .outline()
                            .label(t(L10nKey::Cancel))
                            .disabled(removal.removing)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_worktree_removal(window, cx)
                            })),
                    )
                    .child(
                        Button::new("worktree-removal-remove")
                            .small()
                            .danger()
                            .label(t(if dirty {
                                L10nKey::WorktreePickerRecoverAndRemove
                            } else {
                                L10nKey::WorktreePickerRemove
                            }))
                            .disabled(!removable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_worktree_removal(window, cx)
                            })),
                    ),
            )
            .track_focus(&picker.removal_focus)
    }

    /// The user-facing sentence for a removal block.
    fn worktree_pick_block_text(block: WorktreePickBlock) -> String {
        match block {
            WorktreePickBlock::Gone => t(L10nKey::WorktreePickerRemoveBlockedGone).to_string(),
            WorktreePickBlock::Main => t(L10nKey::WorktreePickerRemoveBlockedMain).to_string(),
            WorktreePickBlock::Changed => t(L10nKey::WorktreePickerRemoveChanged).to_string(),
            WorktreePickBlock::Occupied => {
                t(L10nKey::WorktreePickerRemoveBlockedOccupied).to_string()
            }
            WorktreePickBlock::Unsaved => {
                t(L10nKey::WorktreePickerRemoveBlockedUnsaved).to_string()
            }
            WorktreePickBlock::Unverifiable => {
                t(L10nKey::WorktreePickerRemoveBlockedUnknown).to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use lyrel_ui_base::host_ops::HostId;

    struct WorktreeTestDaemon {
        client: std::sync::Arc<lyrel_core::daemon::control::ControlClient>,
        server: Option<std::thread::JoinHandle<()>>,
        _root: tempfile::TempDir,
    }

    impl gpui_kit::Global for WorktreeTestDaemon {}

    impl Drop for WorktreeTestDaemon {
        fn drop(&mut self) {
            self.client.close();
            if let Some(server) = self.server.take() {
                let _ = server.join();
            }
        }
    }

    fn harness(cx: &mut TestAppContext) -> (Entity<LyrelApp>, VisualTestContext) {
        use lyrel_core::core::machine::MachineStore;
        use lyrel_core::daemon::control::{ControlClient, ControlHello};
        use lyrel_core::host::{local, server};
        use std::net::{TcpListener, TcpStream};
        use std::sync::Arc;

        let root = tempfile::tempdir().unwrap();
        let store = MachineStore::open(root.path().join("machine.json"));
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let connection = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (peer, _) = listener.accept().unwrap();
        let thread = std::thread::spawn(move || {
            server::serve_with(peer, local::new(), server::Services::with_machine(store)).unwrap();
        });
        let hello = ControlHello::host_rpc("worktree-tests", "worktree-test-window");
        let client =
            Arc::new(ControlClient::over_tcp(connection, &hello, Box::new(|_| {})).unwrap());
        let (app, mut vcx) = test_window::harness(cx);
        vcx.update(|_, cx| {
            crate::ui::local_link::LocalLink::install_client_for_test(cx, client.clone());
            cx.set_global(WorktreeTestDaemon {
                client,
                server: Some(thread),
                _root: root,
            });
        });
        (app, vcx)
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    /// HostOps uses a real thread pool, outside the GPUI test executor.
    fn wait_for(
        vcx: &mut VisualTestContext,
        what: &str,
        mut ready: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            settle(vcx);
            if ready(vcx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn open_picker(vcx: &mut VisualTestContext) {
        settle(vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        wait_for(vcx, "both checkout rows", |vcx| row_labels(vcx).len() == 2);
    }

    /// A real repository with one linked worktree: the main checkout at
    /// `main_root`, the linked one at `wt_root` on its own branch.
    fn repository_with_worktree() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, PathBuf) {
        let main = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        let main_root = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(main.path()).unwrap(),
        );
        let wt_root = wt.path().join("linked-checkout");
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
        git(&["init", "--quiet", "--initial-branch=main"], &main_root);
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
        let wt_root = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(wt_root).unwrap(),
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

    fn row_labels(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|window, _| {
            (0u64..LIST_MAX_ROWS as u64)
                .filter_map(|row| window.try_find(("worktree-row", row)))
                .filter(|row| row.visible())
                .filter_map(|row| row.label().map(str::to_owned))
                .collect()
        })
    }

    fn type_in_picker(vcx: &mut VisualTestContext, query: &str) {
        vcx.update(|window, cx| {
            window.press("secondary-a", cx);
            window.input(query, cx);
        });
        settle(vcx);
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

        open_picker(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_picker.is_some()),
            "the status bar Git indicator opens the picker"
        );
        let rows = row_labels(&mut vcx);
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
        type_in_picker(&mut vcx, "wt-branch");
        let by_branch = row_labels(&mut vcx);
        assert_eq!(by_branch.len(), 1);
        assert!(by_branch[0].contains("wt-branch"));
        type_in_picker(&mut vcx, wt_root.parent().unwrap().to_str().unwrap());
        let by_path = row_labels(&mut vcx);
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
        open_picker(&mut vcx);
        type_in_picker(&mut vcx, "wt-branch");
        assert_eq!(row_labels(&mut vcx).len(), 1);
        vcx.simulate_keystrokes("enter");
        wait_for(&mut vcx, "one new terminal tab", |vcx| {
            app.read_with(vcx, |app, _| app.tabs.len() == tabs_before + 1)
        });

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
        open_picker(&mut vcx);
        type_in_picker(&mut vcx, "wt-branch");
        assert_eq!(row_labels(&mut vcx).len(), 1);
        vcx.simulate_keystrokes("enter");
        wait_for(&mut vcx, "focus on the checkout's existing pane", |vcx| {
            vcx.update(|window, cx| right.read(cx).focus_handle(cx).is_focused(window))
        });

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

        open_picker(&mut vcx);
        vcx.update(|window, cx| window.click("worktree-picker-new", cx));
        wait_for(&mut vcx, "the worktree creation card", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find("worktree-prompt-card")
                    .is_some_and(|card| card.visible())
            })
        });
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
                window.try_find("worktree-prompt-card").is_none(),
                "cancelling the prompt closes it"
            );
        });
        assert_eq!(
            app.read_with(&vcx, |app, _| app.tabs.len()),
            tabs_before,
            "cancelling creation must not open a tab"
        );
    }

    /// Run git for a disposable-fixture assertion.
    fn git_at(args: &[&str], cwd: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The same read without the success assertion — "did it happen" checks.
    fn git_maybe(args: &[&str], cwd: &Path) -> Option<String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// The texts the removal dialog shows, scoped to its card.
    fn removal_texts(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|window, _| {
            [
                "worktree-removal-target",
                "worktree-removal-branch",
                "worktree-removal-status",
            ]
            .into_iter()
            .filter_map(|id| window.try_find(id)?.label().map(str::to_owned))
            .collect()
        })
    }

    /// Type the query, then click the footer's removal action for the row it
    /// selects. The dialog's facts arrive from the Host on a real thread
    /// pool; wait_for the expected status text before asserting on it.
    fn open_removal(vcx: &mut VisualTestContext, query: &str) {
        type_in_picker(vcx, query);
        vcx.update(|window, cx| window.click("worktree-picker-remove", cx));
    }

    fn noticed(vcx: &mut VisualTestContext, needle: &str) -> bool {
        vcx.read(|cx| {
            crate::ui::notification_history::history(cx)
                .read(cx)
                .records()
                .any(|record| record.message().contains(needle))
        })
    }

    #[gpui_kit::test]
    fn removing_the_selected_clean_checkout_asks_first_then_removes_and_reports(
        cx: &mut TestAppContext,
    ) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let head = git_at(&["rev-parse", "HEAD"], &wt_root);
        let listed_before = git_at(&["worktree", "list", "--porcelain"], &main_root);
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");

        wait_for(&mut vcx, "the removal dialog's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.update(|window, _| {
            assert!(
                window.find("worktree-removal-card").visible(),
                "the dialog asks before anything is deleted"
            );
        });
        let texts = removal_texts(&mut vcx);
        let path = wt_root.display().to_string();
        assert!(
            texts.iter().any(|text| text.contains(&path)),
            "the dialog identifies the checkout by its path: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.contains("wt-branch") && text.contains("deleted")),
            "the dialog names the branch cleanup: {texts:?}"
        );

        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the checkout to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_picker.is_none())
        });

        assert!(!wt_root.exists(), "the checkout directory is gone");
        let listed_after = git_at(&["worktree", "list", "--porcelain"], &main_root);
        assert!(
            !listed_after.contains(&path),
            "git no longer registers the checkout:\n{listed_after}"
        );
        assert_ne!(listed_before, listed_after);
        assert!(
            git_maybe(
                &["show-ref", "--verify", "--quiet", "refs/heads/wt-branch"],
                &main_root
            )
            .is_none(),
            "the fully merged branch is deleted"
        );
        let name = wt_root.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            git_at(
                &["rev-parse", &format!("refs/lyrel/trash/{name}")],
                &main_root
            ),
            head,
            "the recovery reference holds the checkout's last commit"
        );
        assert!(
            noticed(&mut vcx, "wt-branch") && noticed(&mut vcx, "Removed"),
            "the result notification distinguishes the removed checkout, the branch and the recovery"
        );
        assert!(
            noticed(&mut vcx, "refs/lyrel/trash"),
            "the notification names the recovery reference"
        );
    }

    #[gpui_kit::test]
    fn cancelling_the_removal_dialog_makes_no_changes(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let listed_before = git_at(&["worktree", "list", "--porcelain"], &main_root);
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the removal dialog's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.update(|window, _| {
            assert!(window.find("worktree-removal-card").visible());
        });

        vcx.update(|window, cx| window.click("worktree-removal-cancel", cx));
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| {
                app.worktree_picker
                    .as_ref()
                    .is_some_and(|picker| picker.removal.is_none())
            }),
            "cancel backs out to the picker list"
        );
        vcx.update(|window, _| {
            assert!(
                window.find("worktree-picker-card").visible(),
                "the picker itself is still open"
            );
        });

        // Escape is the other cancellation, from the dialog back to the list.
        open_removal(&mut vcx, "linked-checkout");
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| {
                app.worktree_picker
                    .as_ref()
                    .is_some_and(|picker| picker.removal.is_none())
            }),
            "Escape backs out of the dialog without closing the picker"
        );

        assert_eq!(
            git_at(&["worktree", "list", "--porcelain"], &main_root),
            listed_before,
            "no cancellation touches the registration"
        );
        assert!(wt_root.exists(), "the checkout directory stays");
        assert_eq!(
            git_at(&["for-each-ref", "refs/lyrel/"], &main_root),
            "",
            "no recovery reference is created"
        );
    }

    #[gpui_kit::test]
    fn the_main_checkout_cannot_be_removed(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, _wt_root) = repository_with_worktree();
        let listed_before = git_at(&["worktree", "list", "--porcelain"], &main_root);
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, main_root.file_name().unwrap().to_str().unwrap());
        wait_for(&mut vcx, "the main-checkout refusal", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("main checkout cannot be removed"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("main checkout cannot be removed")),
            "the dialog says why the main checkout is not removable: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_picker.is_some()),
            "the blocked dialog stays open and nothing was removed"
        );
        assert!(main_root.exists());
        assert_eq!(
            git_at(&["worktree", "list", "--porcelain"], &main_root),
            listed_before,
            "the main checkout stays registered exactly as it was"
        );
    }

    #[gpui_kit::test]
    fn dirty_removal_requires_recovery_and_restores_file_bytes(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        std::fs::write(wt_root.join("file.txt"), b"changed\r\n").unwrap();
        std::fs::write(wt_root.join("untracked.txt"), "dirty").unwrap();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(
            &mut vcx,
            "the dirty checkout's recovery confirmation",
            |vcx| {
                removal_texts(vcx)
                    .iter()
                    .any(|text| text.contains("untracked files"))
            },
        );
        vcx.simulate_keystrokes("enter");
        settle(&mut vcx);
        assert!(
            wt_root.exists(),
            "selecting a checkout is not consent to destroy it"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the dirty checkout's recovery reference", |vcx| {
            noticed(vcx, "refs/lyrel/trash/linked-checkout") && !wt_root.exists()
        });
        for (path, expected) in [
            ("file.txt", &b"changed\r\n"[..]),
            ("untracked.txt", &b"dirty"[..]),
        ] {
            let blob = std::process::Command::new("git")
                .current_dir(&main_root)
                .args([
                    "cat-file",
                    "blob",
                    &format!("refs/lyrel/trash/linked-checkout:{path}"),
                ])
                .output()
                .unwrap();
            assert!(blob.status.success());
            assert_eq!(&blob.stdout, expected);
        }
    }

    #[gpui_kit::test]
    fn a_pane_inside_the_checkout_cannot_be_removed(cx: &mut TestAppContext) {
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
        seed_repo_pane(&mut vcx, &right, &wt_root, "wt-branch");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the occupied-checkout refusal", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("A pane is using this checkout"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("A pane is using this checkout")),
            "a pane of this window sitting in the checkout blocks it: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        settle(&mut vcx);
        assert!(wt_root.exists(), "the occupied checkout stays");
    }

    #[gpui_kit::test]
    fn an_unverifiable_occupancy_check_blocks_removal(cx: &mut TestAppContext) {
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
        // The second pane reports a cwd that no longer exists: occupancy
        // cannot be proven either way, and a check that did not run is not
        // proof of absence.
        let vanished = wt_root.join("vanished");
        seed_repo_pane(&mut vcx, &right, &vanished, "wt-branch");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the unverifiable refusal", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("could not be verified"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("could not be verified")),
            "an unverifiable occupancy check blocks the removal: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        settle(&mut vcx);
        assert!(wt_root.exists(), "the unverifiable checkout stays");
    }

    #[gpui_kit::test]
    fn an_unsaved_document_inside_the_checkout_cannot_be_removed(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let file = wt_root.join("file.txt");
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&file, window, cx)
        });
        wait_for(&mut vcx, "the open document", |vcx| {
            app.read_with(vcx, |app, cx| app.active_buffer(cx).is_some())
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.input("unsaved edit", cx);
        });
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, cx| app
                .editor
                .read(cx)
                .is_dirty(HostId::LOCAL, &file)),
            "the fixture holds an unsaved document inside the checkout"
        );

        // The docked document can own the status bar; use the other picker entrance.
        app.update_in(&mut vcx, |app, window, cx| {
            app.set_right_panel_tab(crate::ui::right_panel::RightPanelTab::Scm, window, cx);
        });
        wait_for(&mut vcx, "the Changes worktree entrance", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find("scm-worktree")
                    .is_some_and(|button| button.visible())
            })
        });
        vcx.update(|window, cx| window.click("scm-worktree", cx));
        wait_for(&mut vcx, "both checkouts", |vcx| row_labels(vcx).len() == 2);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the unsaved-document refusal", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("unsaved document"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts.iter().any(|text| text.contains("unsaved document")),
            "the unsaved document blocks the removal: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        settle(&mut vcx);
        assert!(
            wt_root.exists(),
            "the checkout with an unsaved document stays"
        );
    }

    #[gpui_kit::test]
    fn confirmation_revalidates_and_refuses_a_checkout_that_became_dirty(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the removal dialog's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });

        // While the dialog is open, the checkout becomes dirty.
        std::fs::write(wt_root.join("late.txt"), "late").unwrap();
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the post-confirmation refusal", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("uncommitted changes"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("uncommitted changes")),
            "the post-confirmation revalidation refuses with the fresh reason: {texts:?}"
        );
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_picker.is_some()),
            "the refused dialog stays open"
        );
        assert!(wt_root.exists(), "the checkout that became dirty stays");
    }

    #[gpui_kit::test]
    fn same_basename_checkouts_remove_independently_with_distinct_recovery_refs(
        cx: &mut TestAppContext,
    ) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, _wt_root) = repository_with_worktree();
        let scene_a = tempfile::tempdir().unwrap();
        let a = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(scene_a.path()).unwrap(),
        )
        .join("checkout");
        let scene_b = tempfile::tempdir().unwrap();
        let b = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(scene_b.path()).unwrap(),
        )
        .join("checkout");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        git_at(
            &["worktree", "add", "-b", "branch-a", a.to_str().unwrap()],
            &main_root,
        );
        git_at(
            &["worktree", "add", "-b", "branch-b", b.to_str().unwrap()],
            &main_root,
        );
        let head_a = git_at(&["rev-parse", "HEAD"], &a);
        let head_b = git_at(&["rev-parse", "HEAD"], &b);

        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        wait_for(&mut vcx, "all four checkout rows", |vcx| {
            row_labels(vcx).len() == 4
        });

        // The distinguishing parent path selects exactly one of the twins.
        open_removal(&mut vcx, a.parent().unwrap().to_str().unwrap());
        wait_for(&mut vcx, "the first twin's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        let texts = removal_texts(&mut vcx);
        let path_a = a.display().to_string();
        assert!(
            texts.iter().any(|text| text.contains(&path_a)),
            "the dialog identifies which same-basename checkout is targeted: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the first twin to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_picker.is_none())
        });

        assert!(!a.exists(), "the picked checkout is gone");
        assert!(b.exists(), "its same-basename twin stays on disk");
        assert!(
            git_maybe(
                &["show-ref", "--verify", "--quiet", "refs/heads/branch-b"],
                &main_root
            )
            .is_some(),
            "the twin's branch is untouched"
        );
        assert_eq!(
            git_at(&["rev-parse", "refs/lyrel/trash/checkout"], &main_root),
            head_a,
            "the first removal files the plain recovery name"
        );

        // The second twin removes too, under a suffixed recovery name that
        // leaves the first reference intact.
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        wait_for(&mut vcx, "all three remaining checkout rows", |vcx| {
            row_labels(vcx).len() == 3
        });
        open_removal(&mut vcx, b.parent().unwrap().to_str().unwrap());
        wait_for(&mut vcx, "the second twin's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the second twin to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_picker.is_none())
        });

        assert!(!b.exists());
        assert_eq!(
            git_at(&["rev-parse", "refs/lyrel/trash/checkout-2"], &main_root),
            head_b,
            "the second removal takes the suffixed recovery name"
        );
        assert_eq!(
            git_at(&["rev-parse", "refs/lyrel/trash/checkout"], &main_root),
            head_a,
            "the older recovery reference is not overwritten"
        );
    }

    #[gpui_kit::test]
    fn an_unmerged_branch_is_kept_and_said_so(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        std::fs::write(wt_root.join("feature.txt"), "feature").unwrap();
        git_at(&["add", "."], &wt_root);
        git_at(
            &[
                "-c",
                "user.name=lyrel",
                "-c",
                "user.email=lyrel@example.test",
                "commit",
                "-qm",
                "feature",
            ],
            &wt_root,
        );
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the removal dialog's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the checkout to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_picker.is_none())
        });

        assert!(!wt_root.exists(), "the checkout itself is still removed");
        assert!(
            git_maybe(
                &["show-ref", "--verify", "--quiet", "refs/heads/wt-branch"],
                &main_root
            )
            .is_some(),
            "git branch -d refuses the unmerged branch, so it stays"
        );
        assert!(
            noticed(&mut vcx, "kept"),
            "the result notification says the branch was kept"
        );
    }

    #[gpui_kit::test]
    fn a_detached_checkout_is_removed_with_no_branch_work(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        git_at(&["checkout", "-q", "--detach", "HEAD"], &wt_root);
        let branches_before = git_at(&["for-each-ref", "refs/heads/"], &main_root);
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_picker(&mut vcx);
        open_removal(&mut vcx, "linked-checkout");
        wait_for(&mut vcx, "the detached-checkout dialog", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("Detached HEAD"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts.iter().any(|text| text.contains("Detached HEAD")),
            "the dialog names the absent branch cleanup: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the checkout to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_picker.is_none())
        });

        assert!(!wt_root.exists(), "the detached checkout is removed");
        assert_eq!(
            git_at(&["for-each-ref", "refs/heads/"], &main_root),
            branches_before,
            "no branch work ran"
        );
        assert!(
            noticed(&mut vcx, "Recovery reference"),
            "the detached result still names the recovery reference"
        );
        assert!(
            !noticed(&mut vcx, "was deleted"),
            "no deleted-branch claim is made for a detached checkout"
        );
    }
}
