//! GPUI adaptation for Host-aware link interaction. `LinkInteraction` owns
//! targets, hover/probe transitions and retirement; this adapter supplies pane
//! context, schedules HostOps work and delivers navigation/clipboard effects.
use super::menus::menu_row_with_hint;
use super::{
    LoopbackPlan, OpenFileRequested, TerminalView, open_file_path, reveal_file_path,
};
use crate::core::actions::{
    CopyLinkPathUnderPointer, OpenLinkUnderPointer, OpenLinkWithDefaultApp, RevealLinkUnderPointer,
};
use crate::core::config::{Config, LinkFileOpen};
use crate::terminal::link::{LinkAt, LinkInteraction};
use crate::terminal::search::LinkTarget;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::notification_history::NotificationWindowExt as _;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{App, ClipboardItem, Context, Window, prelude::*, px};

enum LoopbackOpen {
    Forwarded(String),
    ForwardFailed(String),
    NotLoopback,
}

impl TerminalView {
    /// Same shape as `warn_paste_upload_failed`: one toast per failed open, so
    /// a broken `link_file_command` surfaces as a config problem instead of a
    /// "dead link" (#542). Spawn is all that is reported — a spawned opener
    /// that exits non-zero is nobody's to see.
    fn warn_file_open_failed(
        &self,
        path: &std::path::Path,
        reason: &std::io::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::warn!("failed to open file link {}: {reason}", path.display());
        let path = path.display().to_string();
        let reason = reason.to_string();
        window.notify_app(
            crate::ui::host_ops::failure(
                crate::ui::i18n::t_fmt(
                    crate::ui::i18n::L10nKey::LinkFileOpenFailed,
                    &[("path", path.as_str()), ("error", reason.as_str())],
                ),
                &reason,
            )
            .pane(self.owner_workspace(), self.pane_id, &self.pane_route()),
            cx,
        );
    }

    pub fn open_link_at(
        &mut self,
        col: usize,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !cx.global::<Config>().link_url {
            return false;
        }
        let include_loopback = self.can_forward_loopback(cx);
        match self.resolve_link_at(col, row, true, include_loopback, cx) {
            LinkAt::Found(LinkTarget::Url(url), ..) => self.open_url(&url, window, cx),
            LinkAt::Found(
                LinkTarget::File {
                    path,
                    line,
                    column,
                    is_dir,
                },
                ..,
            ) => self.open_file_link(path, line, column, is_dir, window, cx),
            LinkAt::Unresolved { candidate, pending } => {
                return self.report_unresolved_link(&candidate, pending, window, cx);
            }
            LinkAt::None => return false,
        }
        true
    }

    /// Latches the file link under a right mouse-down for the context menu.
    ///
    /// Resolving here rather than in the menu builder is what lets the menu
    /// name a real file: the builder runs a turn later, with no event and no
    /// pointer, and asking the grid then would be asking about wherever the
    /// mouse has since gone.
    pub fn record_menu_link(&mut self, col: usize, row: usize, cx: &mut Context<Self>) {
        // The same switch that decides whether a path underlines and whether a
        // click follows one. Without this the menu would go on offering to
        // open files in a pane where link detection is turned off.
        if !cx.global::<Config>().link_url {
            self.links.forget_menu();
            return;
        }
        let include_loopback = self.can_forward_loopback(cx);
        let target = self.resolve_link_at(col, row, true, include_loopback, cx);
        self.links.capture_menu(target);
    }

    /// Drops a latched link, for a right click the application is taking.
    pub fn forget_menu_link(&mut self) {
        self.links.forget_menu();
    }

    /// The path the context menu is about, if it is about one.
    fn menu_link_path(&self) -> Option<&std::path::Path> {
        self.links.menu().map(|link| link.path())
    }

    pub(super) fn open_menu_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.links.menu() else {
            return;
        };
        let mode = link.open_mode(cx.global::<Config>().file_open_mode());
        let host_id = link.host();
        let LinkTarget::File {
            path,
            line,
            column,
            is_dir,
        } = link.target().clone()
        else {
            return;
        };
        self.deliver_file_link(path, line, column, is_dir, host_id, mode, window, cx);
    }

    /// "Open with Default App": the file link under the menu, handed to the
    /// OS association whatever `link_file_open` says "Open" does.
    pub(super) fn open_menu_link_with_default_app(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The menu only offers this for a local pane; a key binding could
        // still fire it on a remote one, where the path means nothing here.
        if !self.links.menu().is_some_and(|link| link.host().is_local()) {
            return;
        }
        let Some(path) = self.menu_link_path().map(std::path::Path::to_path_buf) else {
            return;
        };
        if let Err(e) = open_file_path(&path) {
            self.warn_file_open_failed(&path, &e, window, cx);
        }
    }

    pub(super) fn reveal_menu_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.links.menu().is_some_and(|link| link.host().is_local()) {
            return;
        };
        let Some(path) = self.menu_link_path().map(std::path::Path::to_path_buf) else {
            return;
        };
        if let Err(e) = reveal_file_path(&path) {
            self.warn_file_open_failed(&path, &e, window, cx);
        }
    }

    pub(super) fn copy_menu_link_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.menu_link_path() else {
            return;
        };
        let text = path.to_string_lossy().into_owned();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Hands a resolved file link to whatever the user wants opening files.
    ///
    /// The built-in editor is the default because it is the one place that can
    /// honour `line` and `column`, and the only one that can open a file that
    /// lives on another machine at all — an external command would be handed a
    /// path that means nothing here.
    fn open_file_link(
        &mut self,
        path: std::path::PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode =
            LinkInteraction::file_open_mode(self.host_id, cx.global::<Config>().file_open_mode());
        self.deliver_file_link(path, line, column, is_dir, self.host_id, mode, window, cx);
    }

    fn deliver_file_link(
        &mut self,
        path: std::path::PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        host_id: crate::ui::host_ops::HostId,
        mode: LinkFileOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = cx.global::<Config>().link_file_command.clone();
        // Both external arms have to answer for a failed spawn (#542): a
        // misspelled `link_file_command` or a missing opener used to hit only
        // the log, and the link read as dead — while the path was only ever
        // underlined because it verifiably exists. The built-in editor has
        // its own error path downstream of OpenFileRequested.
        let outcome = match (mode, command) {
            (LinkFileOpen::Command, Some(template)) => {
                run_file_command(&template, &path, line, column)
            }
            (LinkFileOpen::System, _) => open_file_path(&path),
            // Told to run a command, with no command left to run: falling back
            // to the built-in editor beats the click doing nothing.
            (LinkFileOpen::Internal | LinkFileOpen::Command, _) => {
                cx.emit(OpenFileRequested {
                    host_id,
                    path,
                    line,
                    column,
                    is_dir,
                });
                return;
            }
        };
        if let Err(e) = outcome {
            self.warn_file_open_failed(&path, &e, window, cx);
        }
    }

    /// Says why a path-shaped token did not open anything.
    ///
    /// Silence here is what makes the whole feature feel broken: a relative
    /// path measured from somewhere other than this pane's directory looks
    /// exactly like one that works, right up until the click does nothing.
    /// Returns whether the click was spoken for.
    fn report_unresolved_link(
        &mut self,
        candidate: &crate::terminal::search::FileCandidate,
        pending: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(message) = self.links.unresolved_message(candidate, pending) else {
            return false;
        };
        window.notify_app(
            crate::ui::notification_history::Notification::info(message).pane(
                self.owner_workspace(),
                self.pane_id,
                &self.pane_route(),
            ),
            cx,
        );
        true
    }

    fn open_url(&self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.forwarded_loopback_url(url, cx) {
            LoopbackOpen::Forwarded(url) => cx.open_url(&url),
            LoopbackOpen::NotLoopback => cx.open_url(url),
            LoopbackOpen::ForwardFailed(reason) => {
                window.notify_app(
                    crate::ui::notification_history::Notification::info(reason).pane(
                        self.owner_workspace(),
                        self.pane_id,
                        &self.pane_route(),
                    ),
                    cx,
                );
            }
        }
    }

    fn forwarded_loopback_url(&self, url: &str, cx: &mut Context<Self>) -> LoopbackOpen {
        let plan = self.loopback_plan(cx);
        if matches!(plan, LoopbackPlan::Direct) {
            return LoopbackOpen::NotLoopback;
        }
        let Some(loopback) = crate::terminal::loopback::parse_loopback_url(url) else {
            return LoopbackOpen::NotLoopback;
        };
        if matches!(plan, LoopbackPlan::NoForwardNeeded) {
            return LoopbackOpen::NotLoopback;
        }

        let forwarded = match &plan {
            LoopbackPlan::ForwardOnPane(_) | LoopbackPlan::ForwardOnWorkspace(_) => self
                .forward_route()
                .ensure_loopback(loopback.forward_host(), loopback.port),
            LoopbackPlan::Direct | LoopbackPlan::NoForwardNeeded => unreachable!("handled above"),
        };
        match forwarded {
            Ok(forward) => LoopbackOpen::Forwarded(loopback.forwarded_url(forward.local_port)),
            Err(e) => {
                log::warn!("failed to forward loopback URL {url}: {e}");
                LoopbackOpen::ForwardFailed(t_fmt(
                    L10nKey::LoopbackForwardFailed,
                    &[
                        ("port", &loopback.port.to_string()),
                        ("error", &e.to_string()),
                    ],
                ))
            }
        }
    }

    fn can_forward_loopback(&self, cx: &gpui_kit::App) -> bool {
        !matches!(self.loopback_plan(cx), LoopbackPlan::Direct)
    }

    fn sync_link_location(&mut self) {
        self.links.retarget(
            self.host_id,
            self.effective_host_cwd(),
            self.paths_are_local(),
            self.cwd_is_on_host(),
        );
    }

    pub fn hover_link_at(
        &mut self,
        col: usize,
        row: usize,
        armed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(found) = self.links.cached_hover(col, row, armed) {
            return found;
        }
        let alt_screen = self.on_alt_screen();
        if let Some((found, changed)) = self.links.prepare_hover(
            &self.terminal.term,
            col,
            row,
            armed,
            cx.global::<Config>().link_url,
            alt_screen,
        ) {
            if changed {
                cx.notify();
            }
            return found;
        }
        self.sync_link_location();
        let include_loopback = self.can_forward_loopback(cx);
        let (found, changed) = self.links.resolve_hover(
            &self.terminal.term,
            col,
            row,
            armed,
            include_loopback,
            |host, cwd| {
                cx.try_global::<crate::terminal::git_status::GitStatusCache>()
                    .and_then(|cache| cache.repo_root_for(host, cwd))
                    .map(std::path::Path::to_path_buf)
            },
        );
        self.request_link_work(cx);
        self.flush_link_probes(cx);
        if changed {
            cx.notify();
        }
        found
    }

    pub fn refresh_link_hover(&mut self, armed: bool, cx: &mut Context<Self>) -> bool {
        let Some((col, row)) = self.links.refresh_cell(armed, false) else {
            return false;
        };
        self.hover_link_at(col, row, armed, cx)
    }

    fn recompute_link_hover(&mut self, cx: &mut Context<Self>) -> bool {
        let armed = self.links.modifier_down();
        let Some((col, row)) = self.links.refresh_cell(armed, true) else {
            return false;
        };
        self.hover_link_at(col, row, armed, cx)
    }

    pub fn link_modifier_down(&self) -> bool {
        self.links.modifier_down()
    }

    pub fn clear_hovered_link(&mut self, cx: &mut Context<Self>) {
        if self.links.clear_hover() {
            cx.notify();
        }
    }

    fn resolve_link_at(
        &mut self,
        col: usize,
        row: usize,
        include_files: bool,
        include_loopback: bool,
        cx: &mut Context<Self>,
    ) -> LinkAt {
        self.sync_link_location();
        let result = self.links.resolve(
            &self.terminal.term,
            col,
            row,
            include_files,
            include_loopback,
            |host, cwd| {
                cx.try_global::<crate::terminal::git_status::GitStatusCache>()
                    .and_then(|cache| cache.repo_root_for(host, cwd))
                    .map(std::path::Path::to_path_buf)
            },
        );
        self.request_link_work(cx);
        self.flush_link_probes(cx);
        result
    }

    fn request_link_work(&mut self, cx: &mut Context<Self>) {
        if !self.links.wants_root() {
            return;
        }
        // Do not mark work in flight until there is a Host to carry it.
        let Some(host) = self.host(cx) else {
            return;
        };
        if let Some((epoch, cwd)) = self.links.request_root() {
            crate::ui::host_ops::HostOps::run(
                host,
                cx,
                move |h| h.repo_root(&cwd).ok().flatten(),
                move |view, root, cx| {
                    view.sync_link_location();
                    if view.links.land_root(epoch, root) {
                        view.recompute_link_hover(cx);
                    }
                },
            );
        }
    }

    fn flush_link_probes(&mut self, cx: &mut Context<Self>) {
        if self.host_id.is_local() {
            return;
        }
        let Some(host) = self.host(cx) else {
            return;
        };
        let (epoch, wanted) = self.links.take_probes();
        if wanted.is_empty() {
            return;
        }
        crate::ui::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                use crate::terminal::link_probe::Existence;
                wanted
                    .into_iter()
                    .map(|path| {
                        let existence = match h.stat(&path) {
                            Ok(meta) if meta.is_dir => Existence::Dir,
                            Ok(_) => Existence::File,
                            Err(_) => Existence::Missing,
                        };
                        (path, existence)
                    })
                    .collect::<Vec<_>>()
            },
            move |view, answers, cx| {
                view.sync_link_location();
                if view.links.land_probes(epoch, answers) {
                    view.recompute_link_hover(cx);
                }
            },
        );
    }

    /// The captured path is more specific than the pane commands below it.
    pub(super) fn append_link_menu(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        match self.menu_link_path() {
            Some(path) => {
                let local = self.links.menu().is_some_and(|link| link.host().is_local());
                let reveal = match cfg!(target_os = "macos") {
                    true => L10nKey::AppMenuRevealInFinder,
                    false => L10nKey::AppMenuRevealInFolder,
                };
                let label = link_menu_label(path);
                let default_app =
                    offers_default_app_open(local, cx.global::<Config>().file_open_mode());
                menu.min_w(px(220.))
                    .action_context(self.focus_handle.clone())
                    .label(label)
                    .menu(t(L10nKey::AppMenuOpenLink), Box::new(OpenLinkUnderPointer))
                    .when(default_app, |menu| {
                        menu.menu(
                            t(L10nKey::AppMenuOpenLinkWithDefaultApp),
                            Box::new(OpenLinkWithDefaultApp),
                        )
                    })
                    // A file on another machine has no folder here to
                    // show it in, and naming this one's would show
                    // whatever it happens to keep at that path.
                    .menu_element_with_disabled(
                        Box::new(RevealLinkUnderPointer),
                        !local,
                        menu_row_with_hint(t(reveal), None),
                    )
                    .menu(
                        t(L10nKey::AppMenuCopyLinkPath),
                        Box::new(CopyLinkPathUnderPointer),
                    )
                    .separator()
            }
            None => menu,
        }
    }
}

/// The header the link section of the context menu wears: the file's own
/// name, so a menu opened over a long path says which one it is about without
/// making the menu as wide as the path.
fn link_menu_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether a file link's menu offers "Open with Default App" beside "Open".
///
/// Only for a file on this machine: the OS opener is handed a local path, and
/// a remote pane's path names nothing here — or worse, this machine's copy.
/// And not when "Open" already goes to the OS association, where the two rows
/// would do the same thing.
fn offers_default_app_open(host_is_local: bool, open_mode: LinkFileOpen) -> bool {
    host_is_local && open_mode != LinkFileOpen::System
}

fn run_file_command(
    template: &str,
    path: &std::path::Path,
    line: Option<u32>,
    column: Option<u32>,
) -> std::io::Result<()> {
    let argv = expand_file_command_template(template, path, line, column);
    let Some((program, args)) = argv.split_first() else {
        // Sanitize maps a blank template to None, so the only way here is a
        // template whose tokens all expand to nothing (a lone `{line}` on a
        // link with no line number). Still a config error worth reporting.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "link_file_command expanded to nothing",
        ));
    };
    std::process::Command::new(program).args(args).spawn()?;
    Ok(())
}

fn expand_file_command_template(
    template: &str,
    path: &std::path::Path,
    line: Option<u32>,
    column: Option<u32>,
) -> Vec<String> {
    let path = path.to_string_lossy();
    template
        .split_whitespace()
        .filter_map(|token| expand_file_command_token(token, &path, line, column))
        .collect()
}

fn expand_file_command_token(
    token: &str,
    path: &str,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<String> {
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(open) = rest.find('{') {
        let Some(close_rel) = rest[open..].find('}') else {
            break;
        };
        let close = open + close_rel;
        out.push_str(&rest[..open]);
        let value = match &rest[open + 1..close] {
            "path" => Some(path.to_string()),
            "line" => line.map(|l| l.to_string()),
            "column" => column.map(|c| c.to_string()),
            other => Some(format!("{{{other}}}")),
        };
        out.push_str(&value?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod consumer_tests;
