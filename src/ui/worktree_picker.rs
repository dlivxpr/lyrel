//! The shared 工作树 submenu of the two Git menus — the Changes panel's
//! branch menu and the status bar's checkout menu (#351).
//!
//! Both menus append the identical submenu: 新建工作树 first, a separator,
//! then every checkout of the repository as a direct two-line row — the
//! directory name, then the branch (or detached) and the distinguishing
//! parent path — with the menu's own check gutter ticking the checkout the
//! Context Pane sits in. There is deliberately no generic "switch worktree"
//! item: the checkouts themselves are the choices.
//!
//! Picking a checkout navigates: a pane of this window already sitting in
//! the target — its cwd somewhere underneath the checkout root counts — is
//! focused; otherwise exactly one terminal tab opens at the checkout root
//! on the owning Host. Nothing is ever sent to a running process: no `cd`,
//! no branch checkout, no injection. Creating a worktree hands the captured
//! Host and directory to the existing creation prompt, so setup,
//! cancellation and per-worktree commit drafts behave exactly the way they
//! do from every other entrance.
//!
//! Removal (#352) keeps the picker's old route as a discreet trailing
//! submenu: it lists the same checkouts, and clicking one opens the
//! confirmation for exactly that pick. The confirmation re-reads the
//! checkout's current facts on its owning Host before it is offered and
//! again after the confirmation click: the checkout may have become dirty or
//! occupied while the dialog was open. Dirty removal needs its own explicit
//! confirmation and a durable recovery point before Git removes the
//! checkout.

use std::path::{Path, PathBuf};

use gpui_kit::Focusable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, v_flex};
use gpui_kit::{
    AnyElement, App, Context, FocusHandle, IntoElement, KeyDownEvent, MouseButton, SharedString,
    TestSupportExt as _, WeakEntity, Window, div, prelude::*, px, rems,
};

use crate::ui::app::LyrelApp;
use crate::ui::notification_history::NotificationWindowExt as _;
use crate::ui::right_panel::META;
use crate::ui::scm::state::RepoKey;
use lyrel_core::core::worktree::{
    CheckoutInspection, CheckoutRemoval, inspect_checkout, remove_confirmed_checkout,
};
use lyrel_ui_base::host_ops::{HostOps, SharedHost};
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

/// The removal confirmation needs room for a full checkout path and the
/// recovery notice.
const REMOVAL_CARD_W: f32 = 480.;
/// The first line of a checkout row, the same body size the picker used.
const TEXT: f32 = 13. / 16.;
// Keep both menus compact enough to open to the right of the Changes menu.
const MENU_WIDTH: f32 = 12.;
const ROW_TEXT_WIDTH: f32 = 9.5;

/// A checkout the menus list, bound to the Host that owns the repository.
/// Identity is the Host plus the canonical checkout path — never the
/// basename or the branch, which two checkouts can share (#351).
#[derive(Clone)]
pub(crate) struct WorktreePick {
    host: SharedHost,
    checkout: lyrel_core::core::worktree::Listed,
}

/// One checkout's current facts, re-read on its owning Host. Removal (#352)
/// consumes this before offering to delete, and again after the confirmation
/// click: any fact that cannot be read is a block, never a pass, and a fact
/// that changed while the dialog was open blocks with the fresh reason.
struct WorktreePickState {
    /// The pinned identity: the canonical checkout path on its Host. Two
    /// external directories can share a basename, so identity is never the
    /// name and never the branch.
    canonical: PathBuf,
    /// The facts shown by this confirmation, rechecked before deletion.
    inspection: Option<CheckoutInspection>,
    /// What blocks removal right now; `None` while every guard passes.
    block: Option<WorktreePickBlock>,
}

/// Why one checkout cannot be removed from the disk right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreePickBlock {
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

/// The removal confirmation for one pick, shown as the window-level overlay.
/// Pinned at activation — the Host, the checkout identity, the repository
/// directory — and revalidated before the dialog is offered and once more
/// after the confirmation click.
pub(crate) struct WorktreeRemoval {
    id: uuid::Uuid,
    pick: WorktreePick,
    /// The repository directory the revalidation lists checkouts from.
    repo_cwd: PathBuf,
    /// What the open-time revalidation answered; `None` while it runs.
    state: Option<WorktreePickState>,
    /// True from confirmation until its result; ignore repeated activation.
    busy: bool,
    /// Once Git is asked to remove, dismissal cannot cancel that operation.
    removing: bool,
    /// Owns the keyboard while the dialog shows.
    focus: FocusHandle,
}

/// What clicking a checkout row does.
enum WorktreeRowAction {
    /// Navigate: focus a pane already in the checkout, or open one tab.
    Activate,
    /// Remove: open the confirmation for exactly this checkout.
    Remove,
}

/// The Host connection the menus were built against is still the one the
/// registry hands out — a reconnect must not run actions on a stale handle.
pub(crate) fn current_host(expected: &SharedHost, cx: &App) -> bool {
    HostRegistry::lookup(cx, expected.id())
        .is_some_and(|host| std::sync::Arc::ptr_eq(&host, expected))
}

/// The one unavailable row both Git menus show when a Host is missing.
pub(crate) fn unavailable(menu: PopupMenu) -> PopupMenu {
    menu.item(PopupMenuItem::new(t(L10nKey::ScmStatusUnavailable)).disabled(true))
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

fn canonical_checkout(host: &dyn lyrel_core::host::Host, cwd: &Path) -> Option<PathBuf> {
    let out = host
        .git(
            cwd,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .ok()?;
    if !out.success() {
        return None;
    }
    let path = std::str::from_utf8(&out.stdout)
        .ok()?
        .strip_suffix('\n')?
        .trim_end_matches('\r');
    host.canonicalize(Path::new(path)).ok()
}

/// One checkout row of a worktree menu: the directory name on the first
/// line, the branch (or detached) and distinguishing parent path on the
/// second. The menu's own check gutter ticks the current checkout. `kind`
/// separates the navigation rows from the removal submenu's rows so both
/// can be open at once without shared element ids.
#[allow(clippy::too_many_arguments)]
fn worktree_row_item(
    kind: &'static str,
    wt: &lyrel_core::core::worktree::Listed,
    active: bool,
    action: WorktreeRowAction,
    app: WeakEntity<LyrelApp>,
    host: SharedHost,
    repo: RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
) -> PopupMenuItem {
    let name = checkout_name(wt);
    let branch = wt
        .branch
        .clone()
        .unwrap_or_else(|| t(L10nKey::ScmDetached).to_string());
    let path_text = wt.path.display().to_string();
    let second = match wt.path.parent().map(|p| p.display().to_string()) {
        Some(parent) if !parent.is_empty() => format!("{branch} · {parent}"),
        _ => branch.clone(),
    };
    let label: SharedString = if active {
        format!(
            "{name}, {branch}, {path_text}, {}",
            t(L10nKey::WorktreePickerCurrent)
        )
    } else {
        format!("{name}, {branch}, {path_text}")
    }
    .into();
    let row_id: gpui_kit::ElementId =
        gpui_kit::ElementId::Name(format!("{kind}: {path_text}").into());

    let item = PopupMenuItem::element(move |_, cx| {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id(row_id.clone())
            .test_support()
            .role(gpui_kit::Role::MenuItem)
            .aria_label(label.clone())
            .w(rems(ROW_TEXT_WIDTH))
            .overflow_hidden()
            .gap(px(1.))
            .py(px(1.))
            .child(
                div()
                    .w(rems(ROW_TEXT_WIDTH))
                    .truncate()
                    .text_size(rems(TEXT))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(name.clone()),
            )
            .child(
                div()
                    .w(rems(ROW_TEXT_WIDTH))
                    .truncate()
                    .text_ellipsis_middle()
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(second.clone()),
            )
    })
    .checked(active);

    let checkout = wt.clone();
    match action {
        WorktreeRowAction::Activate => item.on_click(move |_, window, cx| {
            let handle = window.window_handle();
            let app = app.clone();
            let host = host.clone();
            let repo = repo.clone();
            let checkout = checkout.clone();
            // Dismissal restores focus to the trigger, and that restore runs
            // after this handler returns. Defer the navigation past the
            // dismiss chain so its focus lands last.
            cx.defer(move |cx| {
                _ = handle.update(cx, |_, window, cx| {
                    _ = app.update(cx, |app, cx| {
                        if !app.status_checkout_is_current(&repo, workspace, window, cx)
                            || !current_host(&host, cx)
                        {
                            return;
                        }
                        // The route is the pane the menu is bound to, not the
                        // window's: a local pane inside a remote workspace
                        // spawns the checkout's terminal locally.
                        let source_workspace = app
                            .context_pane(window, cx)
                            .and_then(|pane| pane.read(cx).workspace().cloned());
                        app.confirm_worktree_pick(
                            WorktreePick { host, checkout },
                            source_workspace,
                            window,
                            cx,
                        );
                    });
                });
            });
        }),
        WorktreeRowAction::Remove => item.on_click(move |_, window, cx| {
            let handle = window.window_handle();
            let app = app.clone();
            let host = host.clone();
            let repo = repo.clone();
            let checkout = checkout.clone();
            cx.defer(move |cx| {
                _ = handle.update(cx, |_, window, cx| {
                    _ = app.update(cx, |app, cx| {
                        if !app.status_checkout_is_current(&repo, workspace, window, cx)
                            || !current_host(&host, cx)
                        {
                            return;
                        }
                        app.begin_worktree_removal(
                            WorktreePick { host, checkout },
                            repo.root.clone(),
                            window,
                            cx,
                        );
                    });
                });
            });
        }),
    }
}

/// The submenu's static head: 新建工作树, then the separator before the
/// checkout list. `PopupMenu::rebuild` clears every item when the read
/// lands, so the landing builder reuses this rather than repeating itself.
fn worktree_menu_shell(
    menu: PopupMenu,
    app: &WeakEntity<LyrelApp>,
    host: &SharedHost,
    repo: &RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
) -> PopupMenu {
    menu.item(PopupMenuItem::new(t(L10nKey::CmdNewWorktreeTab)).on_click({
        let app = app.clone();
        let host = host.clone();
        let repo = repo.clone();
        move |_, window, cx| {
            if !current_host(&host, cx) {
                return;
            }
            _ = app.update(cx, |app, cx| {
                if !app.status_checkout_is_current(&repo, workspace, window, cx) {
                    return;
                }
                // The route is the pane the menu is bound to, not the
                // window's: a local pane inside a remote workspace
                // spawns the worktree terminal locally.
                let source_workspace = app
                    .context_pane(window, cx)
                    .and_then(|pane| pane.read(cx).workspace().cloned());
                app.new_worktree_at(
                    host.clone(),
                    repo.root.clone(),
                    source_workspace,
                    window,
                    cx,
                );
            });
        }
    }))
    .separator()
}

/// Append the shared 工作树 submenu. Both Git menus call this, so they
/// cannot disagree about the choices or the active checkout.
pub(crate) fn worktree_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    app: &WeakEntity<LyrelApp>,
    repo: &RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
    host: Option<&SharedHost>,
) -> PopupMenu {
    let app = app.clone();
    let repo = repo.clone();
    let host = host.cloned();
    menu.submenu(
        t(L10nKey::ScmWorktrees),
        window,
        cx,
        move |menu, window, cx| {
            let Some(host) = host.clone() else {
                return unavailable(menu);
            };
            let expected = host.clone();
            let width = window.rem_size() * MENU_WIDTH;
            let menu = menu.min_w(width).max_w(width);
            let menu = worktree_menu_shell(menu, &app, &host, &repo, workspace);
            let read_repo = repo.clone();
            let land_app = app.clone();
            let land_repo = repo.clone();
            let menu_host = expected.clone();
            let remove_app = app.clone();
            let remove_repo = repo.clone();
            HostOps::run_in(
                expected,
                window,
                cx,
                move |h| -> Result<
                    (Vec<lyrel_core::core::worktree::Listed>, Option<PathBuf>),
                    String,
                > {
                    let worktrees = crate::core::worktree::list(h, &read_repo.root)?;
                    // A managed checkout may live underneath the main checkout.
                    // Git's root, not a prefix match, identifies the current row.
                    let active = canonical_checkout(h, &read_repo.root).and_then(|root| {
                        worktrees
                            .iter()
                            .find(|wt| {
                                h.canonicalize(&wt.path).ok().as_deref() == Some(root.as_path())
                            })
                            .map(|wt| wt.path.clone())
                    });
                    Ok((worktrees, active))
                },
                move |menu, result, window, cx| {
                    // Only the pinned Host and the pinned checkout may fill the
                    // menu; a reconnect or a moved Context Pane leaves it alone.
                    if !current_host(&menu_host, cx)
                        || !land_app.upgrade().is_some_and(|app| {
                            app.read(cx)
                                .status_checkout_is_current(&land_repo, workspace, window, cx)
                        })
                    {
                        return;
                    }
                    menu.rebuild(window, cx, |menu, window, cx| {
                        let menu =
                            worktree_menu_shell(menu, &land_app, &menu_host, &land_repo, workspace);
                        match result {
                            Ok((worktrees, active)) => {
                                let mut menu = menu;
                                for wt in &worktrees {
                                    let is_active = active.as_ref() == Some(&wt.path);
                                    menu = menu.item(worktree_row_item(
                                        "worktree-menu-row",
                                        wt,
                                        is_active,
                                        WorktreeRowAction::Activate,
                                        land_app.clone(),
                                        menu_host.clone(),
                                        land_repo.clone(),
                                        workspace,
                                    ));
                                }
                                // The picker's removal footer survives as one
                                // discreet trailing submenu, scoped away from the
                                // navigation list.
                                menu.separator().submenu(
                                    t(L10nKey::WorktreePickerRemove),
                                    window,
                                    cx,
                                    {
                                        let remove_host = menu_host.clone();
                                        move |menu, window, cx| {
                                            worktree_removal_menu(
                                                menu,
                                                window,
                                                cx,
                                                remove_app.clone(),
                                                remove_repo.clone(),
                                                workspace,
                                                remove_host.clone(),
                                            )
                                        }
                                    },
                                )
                            }
                            Err(error) => menu.item(
                                PopupMenuItem::new(t_fmt(
                                    L10nKey::WorktreePickerFailed,
                                    &[("error", &error)],
                                ))
                                .disabled(true),
                            ),
                        }
                    });
                },
            );
            menu.item(PopupMenuItem::new(t(L10nKey::PanelLoading)).disabled(true))
        },
    )
}

/// The removal entry's submenu: every checkout of the repository, where
/// clicking one opens the removal confirmation for exactly that checkout.
/// The confirmation re-reads the checkout's facts before offering anything.
fn worktree_removal_menu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    app: WeakEntity<LyrelApp>,
    repo: RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
    host: SharedHost,
) -> PopupMenu {
    let width = window.rem_size() * MENU_WIDTH;
    let menu = menu.min_w(width).max_w(width);
    let expected = host.clone();
    let read_repo = repo.clone();
    let land_app = app.clone();
    let land_repo = repo.clone();
    HostOps::run_in(
        host,
        window,
        cx,
        move |h| crate::core::worktree::list(h, &read_repo.root),
        move |menu, result, window, cx| {
            if !current_host(&expected, cx)
                || !land_app.upgrade().is_some_and(|app| {
                    app.read(cx)
                        .status_checkout_is_current(&land_repo, workspace, window, cx)
                })
            {
                return;
            }
            menu.rebuild(window, cx, |menu, _, _| match result {
                Ok(worktrees) => worktrees.iter().fold(menu, |menu, wt| {
                    menu.item(worktree_row_item(
                        "worktree-remove-row",
                        wt,
                        false,
                        WorktreeRowAction::Remove,
                        land_app.clone(),
                        expected.clone(),
                        land_repo.clone(),
                        workspace,
                    ))
                }),
                Err(error) => menu.item(
                    PopupMenuItem::new(t_fmt(L10nKey::WorktreePickerFailed, &[("error", &error)]))
                        .disabled(true),
                ),
            });
        },
    );
    menu.item(PopupMenuItem::new(t(L10nKey::PanelLoading)).disabled(true))
}

impl LyrelApp {
    /// Whether the checkout a menu names is still this window's Context
    /// Pane's: the workspace matches and the pane still reports this
    /// repository. Menus outlive the state they were built from; both
    /// landing and activation ask again.
    pub(crate) fn status_checkout_is_current(
        &self,
        repo: &RepoKey,
        workspace: lyrel_ui_base::session::WorkspaceId,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.workspace == workspace
            && self.context_pane(window, cx).is_some_and(|pane| {
                let view = pane.read(cx);
                view.host_id() == repo.host
                    && view.git_status_cwd().is_some_and(|cwd| {
                        lyrel_ui_base::scm_snapshot::repo_root_for(cx, repo.host, cwd).as_ref()
                            == Some(&repo.root)
                    })
            })
    }

    /// Navigation: focus a pane of this window already sitting in the picked
    /// checkout, or open exactly one terminal tab at its root on the owning
    /// Host. The source pane is never touched — no `cd`, no process moved,
    /// no branch checked out under it.
    fn confirm_worktree_pick(
        &mut self,
        pick: WorktreePick,
        spawn_workspace: Option<lyrel_ui_base::route::PaneWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                let root = h.canonicalize(&match_root).ok()?;
                candidates.iter().find_map(|(tab, leaf, cwd)| {
                    let cwd = h.canonicalize(cwd).ok()?;
                    (cwd.starts_with(&root)
                        && canonical_checkout(h, &cwd).is_some_and(|checkout| checkout == root))
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

    /// The menus' removal action, named for what it does: open the
    /// confirmation for this checkout. Nothing is deleted here; the
    /// confirmation revalidates the pick on its owning Host first.
    pub(crate) fn begin_worktree_removal(
        &mut self,
        pick: WorktreePick,
        repo_cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        self.worktree_removal = Some(WorktreeRemoval {
            id: uuid::Uuid::new_v4(),
            pick: pick.clone(),
            repo_cwd: repo_cwd.clone(),
            state: None,
            busy: false,
            removing: false,
            focus: focus.clone(),
        });
        window.focus(&focus, cx);
        cx.notify();
        self.revalidate_worktree_removal(pick, window, cx);
    }

    /// The removal dialog's dismissal. While Git is removing a checkout the
    /// dialog cannot be cancelled — `open_search` relies on that to keep the
    /// confirmation visible until the operation answers.
    pub(crate) fn cancel_worktree_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(removal) = self.worktree_removal.as_ref() else {
            return;
        };
        if removal.removing {
            return;
        }
        if self.worktree_removal.take().is_some() {
            self.focus_previous_content(window, cx);
            cx.notify();
        }
    }

    /// The removal dialog's confirmation. The facts are re-read on the
    /// owning Host first — the checkout may have become dirty or occupied
    /// while the dialog was open — and only a still-removable checkout runs
    /// the deletion, which itself re-verifies every fact on the Host.
    fn confirm_worktree_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, pick, approved, repo_cwd)) =
            self.worktree_removal.as_mut().and_then(|removal| {
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
                    removal.repo_cwd.clone(),
                ))
            })
        else {
            return;
        };
        cx.notify();
        let host = pick.host.clone();
        let checkout = pick.checkout.clone();
        self.revalidate_worktree_pick(
            pick,
            repo_cwd,
            window,
            cx,
            move |this, mut state, window, cx| {
                let Some(removal) = this.worktree_removal.as_mut() else {
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
            },
        );
    }

    /// The deletion answered: report the distinct facts — the checkout is
    /// gone, the recovery reference, and whether the branch was deleted,
    /// kept or absent — and close the dialog.
    fn finish_worktree_removal(
        &mut self,
        id: uuid::Uuid,
        result: Result<CheckoutRemoval, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, branch) = match self
            .worktree_removal
            .as_ref()
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
                self.worktree_removal = None;
                self.focus_active(window, cx);
                window.notify_app(notice, cx);
                cx.notify();
            }
            Err(e) => {
                if let Some(removal) = self.worktree_removal.as_mut() {
                    removal.busy = false;
                    removal.removing = false;
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

    /// The open-time revalidation: read the pick's facts before the dialog
    /// offers anything. A late answer belongs only to the dialog that asked.
    fn revalidate_worktree_removal(
        &mut self,
        pick: WorktreePick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, repo_cwd)) = self
            .worktree_removal
            .as_ref()
            .map(|removal| (removal.id, removal.repo_cwd.clone()))
        else {
            return;
        };
        self.revalidate_worktree_pick(
            pick,
            repo_cwd,
            window,
            cx,
            move |this, state, _window, cx| {
                let Some(removal) = this.worktree_removal.as_mut() else {
                    return;
                };
                if removal.id == id && !removal.busy {
                    removal.state = Some(state);
                    cx.notify();
                }
            },
        );
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

    pub(crate) fn render_worktree_removal_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let removal = self.worktree_removal.as_ref()?;
        let card = self
            .render_worktree_removal(removal, window, cx)
            .into_any_element();

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(lyrel_ui_base::presets::scrim_fill(cx))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel_worktree_removal(window, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        // The destructive confirmation is still modal.
                        cx.stop_propagation();
                        this.cancel_worktree_removal(window, cx);
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
        crate::ui::dialog::card("worktree-removal-card", title, REMOVAL_CARD_W, cx)
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
            .track_focus(&removal.focus)
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
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Entity, MouseButton, TestAppContext, VisualTestContext};
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
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
    /// (branch + counts) the status bar and panel read from.
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

    /// Open the status bar's checkout menu.
    fn open_status_checkout_menu(vcx: &mut VisualTestContext) {
        settle(vcx);
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        settle(vcx);
    }

    /// Hover a menu item by label so its submenu opens.
    fn hover_menu_item(vcx: &mut VisualTestContext, label: &str) {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let item = window.find_by_label(label);
            window.pointer_move(item.bounds().center(), None, cx);
        });
        settle(vcx);
    }

    /// Click a plain menu item by its label.
    fn click_menu_item(vcx: &mut VisualTestContext, label: &str) {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let item = window.find_by_label(label);
            let position = item.bounds().center();
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
        });
        settle(vcx);
    }

    /// A checkout row's element id — the same composition the row uses, so
    /// the spelling git answers with is the spelling the test asks for.
    fn row_id(kind: &str, path: &Path) -> gpui_kit::ElementId {
        gpui_kit::ElementId::Name(format!("{kind}: {}", path.display()).into())
    }

    /// Click one checkout row of an open (sub)menu.
    fn click_checkout_row(vcx: &mut VisualTestContext, kind: &str, path: &Path) {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let row = window.find(row_id(kind, path));
            let position = row.bounds().center();
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
        });
    }

    /// Open the checkout menu and hover into the shared 工作树 submenu.
    fn open_worktree_menu(vcx: &mut VisualTestContext) {
        open_status_checkout_menu(vcx);
        hover_menu_item(vcx, t(L10nKey::ScmWorktrees));
    }

    /// Wait for both checkout rows of the shared submenu.
    fn wait_for_checkout_rows(vcx: &mut VisualTestContext, main_root: &Path, wt_root: &Path) {
        wait_for(vcx, "both checkout rows", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find(row_id("worktree-menu-row", main_root))
                    .is_some()
                    && window
                        .try_find(row_id("worktree-menu-row", wt_root))
                        .is_some()
            })
        });
    }

    /// Hover the discreet removal entry inside the open 工作树 submenu.
    fn open_removal_entry(vcx: &mut VisualTestContext) {
        hover_menu_item(vcx, t(L10nKey::WorktreePickerRemove));
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

    /// Open the removal confirmation for `path` through the status bar menu.
    fn begin_removal(vcx: &mut VisualTestContext, path: &Path) {
        wait_for(vcx, "the removal row", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find(row_id("worktree-remove-row", path))
                    .is_some()
            })
        });
        click_checkout_row(vcx, "worktree-remove-row", path);
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

    fn noticed(vcx: &mut VisualTestContext, needle: &str) -> bool {
        vcx.read(|cx| {
            crate::ui::notification_history::history(cx)
                .read(cx)
                .records()
                .any(|record| record.message().contains(needle))
        })
    }

    #[gpui_kit::test]
    fn the_menu_lists_checkouts_and_ticks_the_active_one(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");
        app.update_in(&mut vcx, |app, window, cx| {
            window.focus(&app.status_checkout_focus, cx);
        });

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_removal.is_none()),
            "listing checkouts never touches the removal confirmation"
        );
        vcx.update(|window, _| {
            let main_row = window.find(row_id("worktree-menu-row", &main_root));
            assert!(
                main_row
                    .label()
                    .is_some_and(|label| label.ends_with(", current")),
                "the check gutter marks the checkout the Context Pane sits in"
            );
            let linked_row = window.find(row_id("worktree-menu-row", &wt_root));
            let label = linked_row.label().expect("the row announces its name");
            assert!(
                label.contains("wt-branch"),
                "the branch is on the row: {label}"
            );
            assert!(
                !label.contains(", current"),
                "only the active checkout carries the tick: {label}"
            );
        });

        // Escape closes the menu without navigating.
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, _| app.worktree_removal.is_none()));
        vcx.update(|window, _| {
            assert!(
                window
                    .try_find(row_id("worktree-menu-row", &wt_root))
                    .is_none()
            );
            assert_eq!(window.find("status-git-branch").focused(), Some(true));
        });
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
        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        click_checkout_row(&mut vcx, "worktree-menu-row", &wt_root);
        wait_for(&mut vcx, "one new terminal tab", |vcx| {
            app.read_with(vcx, |app, _| app.tabs.len() == tabs_before + 1)
        });

        assert!(
            app.read_with(&vcx, |app, _| app.worktree_removal.is_none()),
            "navigating never opens the removal confirmation"
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
        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        click_checkout_row(&mut vcx, "worktree-menu-row", &wt_root);
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
    fn create_from_the_menu_opens_the_existing_prompt_and_cancels_cleanly(cx: &mut TestAppContext) {
        lyrel_ui_base::i18n::set_locale("en");
        let (_main, _wt, main_root, _wt_root) = repository_with_worktree();
        let (app, mut vcx) = harness(cx);
        let (pane, _stream) = add_repo_tab(&app, &mut vcx, 1);
        app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.select(0);
            cx.notify();
        });
        seed_repo_pane(&mut vcx, &pane, &main_root, "main");

        open_worktree_menu(&mut vcx);
        click_menu_item(&mut vcx, t(L10nKey::CmdNewWorktreeTab));
        wait_for(&mut vcx, "the worktree creation card", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find("worktree-prompt-card")
                    .is_some_and(|card| card.visible())
            })
        });
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_removal.is_none()),
            "creating hands over to the existing creation prompt"
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);

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
            app.read_with(vcx, |app, _| app.worktree_removal.is_none())
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
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
            app.read_with(&vcx, |app, _| app.worktree_removal.is_none()),
            "cancel closes the dialog"
        );

        // Escape is the other cancellation. The first dialog's row click
        // dismissed the menu chain, so the whole menu opens again.
        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
        wait_for(&mut vcx, "the removal dialog's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.worktree_removal.is_none()),
            "Escape closes the dialog without removing anything"
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

        open_worktree_menu(&mut vcx);
        wait_for(&mut vcx, "the checkout rows", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find(row_id("worktree-menu-row", &main_root))
                    .is_some()
            })
        });
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &main_root);
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
            app.read_with(&vcx, |app, _| app.worktree_removal.is_some()),
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
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
        assert!(app.read_with(&vcx, |app, _| app.worktree_removal.is_some()));
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the recovery attempt to finish", |vcx| {
            app.read_with(vcx, |app, _| {
                app.worktree_removal
                    .as_ref()
                    .is_none_or(|removal| !removal.busy)
            })
        });
        let notices = vcx.read(|cx| {
            crate::ui::notification_history::history(cx)
                .read(cx)
                .records()
                .map(|record| record.message().to_owned())
                .collect::<Vec<_>>()
        });
        assert!(
            noticed(&mut vcx, "refs/lyrel/trash/linked-checkout") && !wt_root.exists(),
            "the confirmed dirty checkout is removed with recovery; dialog: {:?}; notices: {notices:?}",
            removal_texts(&mut vcx)
        );
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
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

        // The docked document can own the status bar; use the panel's branch menu.
        app.update_in(&mut vcx, |app, window, cx| {
            app.set_right_panel_tab(lyrel_core::core::config::RightPanelTab::Scm, window, cx);
        });
        wait_for(&mut vcx, "the Changes branch menu", |vcx| {
            vcx.update(|window, _| {
                window
                    .try_find("scm-branch")
                    .is_some_and(|button| button.visible())
            })
        });
        vcx.update(|window, cx| window.click("scm-branch", cx));
        settle(&mut vcx);
        hover_menu_item(&mut vcx, t(L10nKey::ScmWorktrees));
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
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
            app.read_with(&vcx, |app, _| app.worktree_removal.is_some()),
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

        // The distinguishing full path selects exactly one of the twins.
        open_worktree_menu(&mut vcx);
        wait_for(&mut vcx, "all four checkout rows", |vcx| {
            vcx.update(|window, _| {
                window.try_find(row_id("worktree-menu-row", &a)).is_some()
                    && window.try_find(row_id("worktree-menu-row", &b)).is_some()
            })
        });
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &a);
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
            app.read_with(vcx, |app, _| app.worktree_removal.is_none())
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
        // leaves the first reference intact. The first removal's row click
        // dismissed the menu chain, so the whole menu opens again.
        open_worktree_menu(&mut vcx);
        wait_for(&mut vcx, "all three remaining checkout rows", |vcx| {
            vcx.update(|window, _| {
                window.try_find(row_id("worktree-menu-row", &b)).is_some()
                    && window
                        .try_find(row_id("worktree-menu-row", &main_root))
                        .is_some()
            })
        });
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &b);
        wait_for(&mut vcx, "the second twin's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the second twin to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_removal.is_none())
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
        wait_for(&mut vcx, "the removal dialog's facts", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the checkout to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_removal.is_none())
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

        open_worktree_menu(&mut vcx);
        wait_for_checkout_rows(&mut vcx, &main_root, &wt_root);
        open_removal_entry(&mut vcx);
        begin_removal(&mut vcx, &wt_root);
        wait_for(&mut vcx, "the detached checkout to be verified", |vcx| {
            removal_texts(vcx)
                .iter()
                .any(|text| text.contains("clean and unused"))
        });

        let texts = removal_texts(&mut vcx);
        assert!(
            texts.iter().any(|text| text.contains("Detached HEAD")),
            "the dialog names the absent branch cleanup: {texts:?}"
        );
        vcx.update(|window, cx| window.click("worktree-removal-remove", cx));
        wait_for(&mut vcx, "the checkout to be removed", |vcx| {
            app.read_with(vcx, |app, _| app.worktree_removal.is_none())
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
