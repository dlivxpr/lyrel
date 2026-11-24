//! The status bar's Git menu is scoped to the checkout named by its trigger.

use std::hash::{Hash as _, Hasher as _};
use std::io;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::{
    Anchor, AnyElement, ClipboardItem, Context, WeakEntity, Window, div, prelude::*, rems,
};
use lyrel_core::core::git::{
    log::local_branches,
    ops::{GIT_NETWORK_DEADLINE, GitOp},
    status::{HeadState, StatusProbe, WorkingTreeStatus, probe_status},
};
use lyrel_core::host::{Host, Output};
use lyrel_ui_base::host_ops::{HostOps, SharedHost};
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use crate::ui::app::LyrelApp;
use crate::ui::notification_history::{Notification, NotificationWindowExt as _};
use crate::ui::scm::state::RepoKey;
use crate::ui::worktree_picker::{current_host, unavailable, worktree_submenu};

impl LyrelApp {
    pub(super) fn status_checkout_trigger(
        &self,
        button: Button,
        repo: RepoKey,
        branch: gpui_kit::SharedString,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = button.track_focus(&self.status_checkout_focus);
        let app = cx.entity().downgrade();
        let host = HostRegistry::lookup(cx, repo.host);
        let workspace = self.workspace;
        let mut key = std::hash::DefaultHasher::new();
        (
            workspace,
            &repo,
            &branch,
            host.as_ref().map(|host| Arc::as_ptr(host) as *const ()),
        )
            .hash(&mut key);
        let dropdown =
            button.dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, cx| {
                checkout_items(
                    menu,
                    window,
                    cx,
                    &app,
                    &repo,
                    workspace,
                    host.as_ref(),
                    branch.clone(),
                )
            });
        div()
            .id(("status-checkout-context", key.finish()))
            .flex_shrink_1()
            .min_w(rems(2.))
            .max_w(rems(22.))
            .child(dropdown)
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn checkout_items(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    app: &WeakEntity<LyrelApp>,
    repo: &RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
    host: Option<&SharedHost>,
    branch: gpui_kit::SharedString,
) -> PopupMenu {
    let folder = repo.root.file_name().unwrap_or(repo.root.as_os_str());
    let menu = menu
        .min_w(window.rem_size() * 16.)
        .label(folder.to_string_lossy().into_owned());

    let menu = menu.submenu(branch, window, cx, {
        let app = app.clone();
        let repo = repo.clone();
        let host = host.cloned();
        move |menu, window, cx| {
            let Some(host) = host.clone() else {
                return unavailable(menu);
            };
            let root = repo.root.clone();
            let repo = repo.clone();
            let app = app.clone();
            let expected = host.clone();
            HostOps::run_in(
                host,
                window,
                cx,
                move |host| match probe_status(host, &root) {
                    StatusProbe::Status(status) => Some((status, local_branches(host, &root))),
                    _ => None,
                },
                move |menu, data, window, cx| {
                    if !current_host(&expected, cx)
                        || !app.upgrade().is_some_and(|app| {
                            app.read(cx)
                                .status_checkout_is_current(&repo, workspace, window, cx)
                        })
                    {
                        return;
                    }
                    menu.rebuild(window, cx, |menu, window, _| match data {
                        Some((status, branches)) => branch_items(
                            menu, window, &app, &repo, workspace, &expected, &status, branches,
                        ),
                        None => unavailable(menu),
                    });
                },
            );
            menu.item(PopupMenuItem::new(t(L10nKey::PanelLoading)).disabled(true))
        }
    });
    // 工作树 sits directly below the branch submenu; the file actions'
    // leading separator keeps 查看文件/复制 on the far side of it.
    let menu = menu.separator();
    let menu = worktree_submenu(menu, window, cx, app, repo, workspace, host);
    checkout_file_actions(menu, window, cx, app, repo, workspace, host)
}

#[allow(clippy::too_many_arguments)]
fn branch_items(
    mut menu: PopupMenu,
    window: &Window,
    app: &WeakEntity<LyrelApp>,
    repo: &RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
    host: &SharedHost,
    status: &WorkingTreeStatus,
    branches: Vec<String>,
) -> PopupMenu {
    menu = menu.label(t(L10nKey::StatusCheckoutBranchInPlace));
    if let HeadState::Branch { oid, .. } | HeadState::Detached { oid } = &status.head {
        menu = menu.label(oid.chars().take(7).collect::<String>());
    }
    if branches.len() > 12 {
        menu = menu.scrollable(true).max_h(window.rem_size() * 20.);
    }
    for name in &branches {
        let current = match &status.head {
            HeadState::Branch { name: current, .. } => current == name,
            _ => false,
        };
        menu = menu.item(
            PopupMenuItem::new(name.clone())
                .checked(current)
                .disabled(current)
                .on_click({
                    let app = app.clone();
                    let host = host.clone();
                    let repo = repo.clone();
                    let name = name.clone();
                    move |_, window, cx| {
                        if current_host(&host, cx) {
                            _ = app.update(cx, |app, cx| {
                                if !app.status_checkout_is_current(&repo, workspace, window, cx) {
                                    return;
                                }
                                app.scm_op(
                                    repo.clone(),
                                    GitOp::CheckoutBranch { name: name.clone() },
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                }),
        );
    }
    let check = match (&status.head, &status.upstream) {
        (HeadState::Branch { name, oid }, Some(upstream)) => {
            let app = app.clone();
            let repo = repo.clone();
            let root = repo.root.clone();
            let host = host.clone();
            let name = name.clone();
            let oid = oid.clone();
            let upstream = upstream.clone();
            PopupMenuItem::new(t(L10nKey::StatusCheckoutCheckUpstream)).on_click(
                move |_, window, cx| {
                    _ = app.update(cx, |app, cx| {
                        if current_host(&host, cx)
                            && app.status_checkout_is_current(&repo, workspace, window, cx)
                        {
                            let (root, name, oid, upstream) =
                                (root.clone(), name.clone(), oid.clone(), upstream.clone());
                            let expected = host.clone();
                            let repo = repo.clone();
                            let upstream_for_host = upstream.clone();
                            HostOps::run_in(
                                host.clone(),
                                window,
                                cx,
                                move |host| {
                                    upstream_difference(
                                        host,
                                        &root,
                                        &name,
                                        &oid,
                                        &upstream_for_host,
                                    )
                                },
                                move |app, result, window, cx| {
                                    if !app.status_checkout_is_current(&repo, workspace, window, cx)
                                        || !current_host(&expected, cx)
                                    {
                                        return;
                                    }
                                    let notice = match result {
                                        Ok(Some((ahead, behind))) => Notification::info(t_fmt(
                                            L10nKey::StatusCheckoutUpstreamResult,
                                            &[
                                                ("upstream", &upstream),
                                                ("ahead", &ahead.to_string()),
                                                ("behind", &behind.to_string()),
                                            ],
                                        )),
                                        Ok(None) => return,
                                        Err(err) => {
                                            HostOps::notify_err(
                                                window,
                                                cx,
                                                t(L10nKey::StatusCheckoutUpstreamFailed),
                                                &err,
                                            );
                                            return;
                                        }
                                    };
                                    window.notify_app(notice, cx);
                                },
                            );
                        }
                    });
                },
            )
        }
        _ => PopupMenuItem::new(t(L10nKey::StatusCheckoutNoUpstream)).disabled(true),
    };
    menu.separator().item(check)
}

fn checkout_file_actions(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    app: &WeakEntity<LyrelApp>,
    repo: &RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
    host: Option<&SharedHost>,
) -> PopupMenu {
    let path = if repo.host.is_local() {
        lyrel_ui_base::path_display::native_separators(&repo.root)
            .display()
            .to_string()
    } else {
        repo.root.display().to_string()
    };
    let terminal_directory = {
        let app = app.clone();
        let repo = repo.clone();
        let host = host.cloned();
        move |window: &Window, cx: &gpui_kit::App| {
            if !repo.host.is_local() {
                return None;
            }
            let host = host.as_ref().filter(|host| current_host(host, cx))?;
            let app = app.upgrade()?;
            let app = app.read(cx);
            if !app.status_checkout_is_current(&repo, workspace, window, cx) {
                return None;
            }
            app.context_pane(window, cx)?
                .read(cx)
                .host_cwd()
                .filter(|dir| host.is_absolute(dir))
        }
    };
    menu.separator()
        .item(
            PopupMenuItem::new(t(L10nKey::StatusCheckoutViewFiles))
                .disabled(terminal_directory(window, cx).is_none())
                .on_click(move |_, window, cx| {
                    let Some(dir) = terminal_directory(window, cx) else {
                        return;
                    };
                    if let Err(err) = lyrel_ui_base::desktop_file::open_file_path(&dir) {
                        let message = t_fmt(
                            L10nKey::OpenInFileManagerFailed,
                            &[("path", &dir.display().to_string())],
                        );
                        HostOps::notify_err(window, cx, &message, &err);
                    }
                }),
        )
        .submenu(t(L10nKey::StatusCheckoutCopy), window, cx, {
            let app = app.clone();
            let repo = repo.clone();
            let host = host.cloned();
            move |menu, _, _| {
                let head = |label, args| {
                    copy_head_item(
                        label,
                        args,
                        app.clone(),
                        repo.clone(),
                        workspace,
                        host.clone(),
                    )
                };
                menu.item(copy_item(L10nKey::StatusCheckoutCopyPath, path.clone()))
                    .item(head(
                        L10nKey::StatusCheckoutCopyBranch,
                        &["symbolic-ref", "--quiet", "--short", "HEAD"],
                    ))
                    .item(head(
                        L10nKey::ScmCopyCommitSha,
                        &["rev-parse", "--verify", "HEAD"],
                    ))
            }
        })
}

fn copy_item(label: L10nKey, value: String) -> PopupMenuItem {
    PopupMenuItem::new(t(label)).on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
    })
}

fn copy_head_item(
    label: L10nKey,
    args: &'static [&'static str],
    app: WeakEntity<LyrelApp>,
    repo: RepoKey,
    workspace: lyrel_ui_base::session::WorkspaceId,
    host: Option<SharedHost>,
) -> PopupMenuItem {
    PopupMenuItem::new(t(label))
        .disabled(host.is_none())
        .on_click(move |_, window, cx| {
            let Some(host) = host.as_ref().filter(|host| current_host(host, cx)) else {
                return;
            };
            _ = app.update(cx, |app, cx| {
                if !app.status_checkout_is_current(&repo, workspace, window, cx) {
                    return;
                }
                let root = repo.root.clone();
                let repo = repo.clone();
                let expected = host.clone();
                HostOps::run_in(
                    host.clone(),
                    window,
                    cx,
                    move |host| {
                        let out = host.git(&root, args).ok()?;
                        out.success()
                            .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
                    },
                    move |app, value, window, cx| {
                        if current_host(&expected, cx)
                            && app.status_checkout_is_current(&repo, workspace, window, cx)
                        {
                            match value {
                                Some(value) => {
                                    cx.write_to_clipboard(ClipboardItem::new_string(value))
                                }
                                None => window.notify_app(
                                    Notification::error(t(L10nKey::ScmStatusUnavailable)),
                                    cx,
                                ),
                            }
                        }
                    },
                );
            });
        })
}

/// Compare against the ref currently advertised by the configured upstream,
/// not a possibly stale tracking ref. Fetch only to obtain history objects;
/// no checkout, merge, push, tracking-ref update or shared FETCH_HEAD write.
fn upstream_difference(
    host: &dyn Host,
    root: &Path,
    name: &str,
    oid: &str,
    upstream: &str,
) -> io::Result<Option<(u32, u32)>> {
    let branch = format!("refs/heads/{name}");
    let read_branch = || {
        git_stdout(host.git(root, &[
            "for-each-ref",
            "--format=%(HEAD)%00%(objectname)%00%(upstream:short)%00%(upstream:remotename)%00%(upstream:remoteref)",
            &branch,
        ])?)
    };
    let start = read_branch()?;
    let mut fields = start.trim_end().split('\0');
    if fields.next() != Some("*") || fields.next() != Some(oid) || fields.next() != Some(upstream) {
        return Ok(None);
    }
    let remote = fields
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid_upstream)?;
    let remote_ref = fields
        .next()
        .filter(|s| s.starts_with("refs/heads/"))
        .ok_or_else(invalid_upstream)?;
    let result = if remote == "." {
        difference_between(host, root, oid, remote_ref)
    } else {
        remote_difference(host, root, oid, remote, remote_ref)
    };
    // A commit, branch switch or upstream change does not change the path.
    match read_branch() {
        Ok(end) if end == start => result.map(Some),
        Ok(_) => Ok(None),
        Err(err) => result.and(Err(err)),
    }
}

fn remote_difference(
    host: &dyn Host,
    root: &Path,
    oid: &str,
    remote: &str,
    remote_ref: &str,
) -> io::Result<(u32, u32)> {
    let remote_tip = git_stdout(host.git_with_deadline(
        root,
        &[
            "ls-remote",
            "--exit-code",
            "--refs",
            "--",
            remote,
            remote_ref,
        ],
        GIT_NETWORK_DEADLINE,
    )?)?;
    let (tip, target) = remote_tip
        .trim_end()
        .split_once('\t')
        .ok_or_else(invalid_upstream)?;
    if target != remote_ref
        || !matches!(tip.len(), 40 | 64)
        || !tip.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid_upstream());
    }
    git_stdout(host.git_with_deadline(
        root,
        &[
            "fetch",
            "--no-write-fetch-head",
            "--no-tags",
            "--no-prune",
            "--no-recurse-submodules",
            "--refmap=",
            "--",
            remote,
            tip,
        ],
        GIT_NETWORK_DEADLINE,
    )?)?;
    difference_between(host, root, oid, tip)
}

fn difference_between(
    host: &dyn Host,
    root: &Path,
    oid: &str,
    tip: &str,
) -> io::Result<(u32, u32)> {
    let range = format!("{oid}...{tip}");
    let counts =
        git_stdout(host.git(root, &["rev-list", "--left-right", "--count", &range, "--"])?)?;
    let mut counts = counts.split_whitespace();
    let mut next = || {
        counts
            .next()
            .and_then(|n| n.parse().ok())
            .ok_or_else(invalid_upstream)
    };
    Ok((next()?, next()?))
}

fn git_stdout(output: Output) -> io::Result<String> {
    if !output.success() {
        return Err(io::Error::other(match output.status {
            Some(code) => format!("Git exited with status {code}"),
            None => "Git did not report an exit status".to_owned(),
        }));
    }
    String::from_utf8(output.stdout).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn invalid_upstream() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Git returned invalid upstream data",
    )
}
