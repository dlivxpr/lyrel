//! Title, bell and notification adaptation for the retained terminal entity.
//! AgentActivity remains authoritative for read marks and cancellable Done notices.

use super::*;
use gpui_kit::component::ActiveTheme as _;

const INTEGRATION_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

const INTEGRATION_NOTICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// How long a title the program set has to stand before the tab adopts it.
///
/// Long enough that a command which is over almost as soon as it started never
/// reaches the label, short enough that one still running is named while the
/// wait for it is still what the reader is doing.
const TITLE_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// What a pane does with a title the program just set — see
/// `TerminalView::set_title_when_settled`.
#[derive(Debug, PartialEq, Eq)]
enum TitleSettle {
    /// The tab already reads this, so whatever was waiting to replace it never
    /// has to happen. This is the case a short command lands in, and the one an
    /// agent's spinner lands in: a frame that only changes the status mark in
    /// front of the title reads the same on the tab, so it asks for no repaint.
    Revert,
    /// Hold it: a wait is already running and adopts the newest title when it
    /// elapses. Restarting the wait instead would let a program that rewrites
    /// its title faster than the wait — a download reporting progress — put
    /// off the tab's next update forever.
    Queue,
    /// Hold it and start the wait.
    QueueAndWait,
}

fn settle_title(showing: &str, waiting: bool, incoming: &str) -> TitleSettle {
    if lyrel_core::core::tab_view::same_title_ignoring_status_mark(incoming, showing) {
        return TitleSettle::Revert;
    }
    match waiting {
        true => TitleSettle::Queue,
        false => TitleSettle::QueueAndWait,
    }
}

fn known_pty_shim(fg: &str) -> Option<&'static str> {
    ["kiro-cli-term", "figterm", "qterm", "cwterm"]
        .into_iter()
        .find(|shim| fg.contains(shim))
}

fn integration_notice_message(wrapper: Option<&str>) -> String {
    match wrapper {
        Some(w) => t_fmt(L10nKey::IntegrationNoticeBlocked, &[("wrapper", w)]),
        None => t(L10nKey::IntegrationNoticeNotEngaged).to_string(),
    }
}

/// Join whatever context a pane has into a notification title, most specific
/// part first: an agent name if one is running, otherwise the machine the pane
/// lives on, then the workspace it belongs to. Kept to two segments — Windows
/// toast titles are a single line and ellipsize anything longer.
fn compose_notification_title(
    lead: Option<String>,
    host: Option<String>,
    workspace: Option<String>,
) -> String {
    match (lead.or(host), workspace) {
        (Some(lead), Some(workspace)) => format!("{lead} · {workspace}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => "lyrel".to_string(),
    }
}

impl TerminalView {
    pub(super) fn notify_command_finished(
        &self,
        label: &str,
        elapsed: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        let secs = elapsed.as_secs().to_string();
        let command = label.trim();
        let body = if command.is_empty() {
            t_fmt(L10nKey::NotifyCommandFinished, &[("secs", &secs)])
        } else {
            t_fmt(
                L10nKey::NotifyCommandFinishedWithCommand,
                &[("command", command), ("secs", &secs)],
            )
        };
        self.notify_pane(None, &body, cx);
    }

    pub(super) fn notify_agent_finished(
        &self,
        agent: crate::core::cli_agent::CLIAgent,
        elapsed: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        let secs = elapsed.as_secs().to_string();
        let body = t_fmt(L10nKey::NotifyAgentFinished, &[("secs", &secs)]);
        self.notify_pane(Some(agent.display_name()), &body, cx);
    }

    /// Notify about this pane, with a title describing where it lives and a
    /// click that reveals it. The id has to be the pane's *gpui entity id*:
    /// that is what `TrayAction::RevealPane` matches leaves on, and it is a
    /// different number from `pane_id`, which the daemon assigns.
    pub(super) fn notify_pane(&self, lead: Option<&str>, body: &str, cx: &mut Context<Self>) {
        let title = self.notification_title(lead, cx);
        crate::terminal::desktop_notification::notify_desktop_for_pane(
            Some(&title),
            body,
            Some(cx.entity_id()),
        );
    }

    /// Desktop notifications the program wrote (OSC 9, 99, 777), such as
    /// Claude Code's with its Notifications setting on `ghostty`, `kitty` or
    /// `iterm2`. The agent's Waiting mark on the tab is the daemon's doing.
    ///
    /// `Unfocused` holds a note back only while the reader is looking at this
    /// very pane. A pane whose agent reports through lyrel's hooks already gets
    /// its Waiting and Done notices from `poll_agent_status` whenever
    /// `hooks_notify`, so the program's own copy would be a duplicate there.
    ///
    /// At most a few per pane every few seconds reach the desktop (pane output
    /// is untrusted), with one note saying the rest were not shown.
    pub(super) fn show_program_notes(&self, hooks_notify: bool, window: &Window, cx: &mut Context<Self>) {
        let notes = self.terminal.take_osc_notes();
        let watched = window.is_window_active() && self.focus_handle.is_focused(window);
        let hooked = self.terminal.agent_session().is_some_and(|s| s.rich);
        let show = cx
            .global::<Config>()
            .notify_on_command_finish
            .allows(watched)
            && !(hooked && hooks_notify);
        if !notes.is_empty() {
            log::debug!(
                "{} program notification(s) {}",
                notes.len(),
                if show { "shown" } else { "held back" }
            );
        }
        // Asked on every poll, so the rest are said once a flood stops too,
        // and while notes are held back, so a stale count never surfaces
        // minutes later.
        let (notes, dropped) = self.terminal.pace_osc_notes(
            if show { notes } else { Vec::new() },
            std::time::Instant::now(),
        );
        if !show {
            return;
        }
        let agent = self.terminal.foreground_agent().map(|a| a.display_name());
        if dropped {
            self.notify_pane(agent, t(L10nKey::ProgramNotesDropped), cx);
        }
        for (title, body) in notes {
            match title {
                Some(title) => crate::terminal::desktop_notification::notify_desktop_for_pane(
                    Some(&title),
                    &body,
                    Some(cx.entity_id()),
                ),
                None => self.notify_pane(agent, &body, cx),
            }
        }
    }

    pub(super) fn notification_title(&self, lead: Option<&str>, cx: &App) -> String {
        let host = self
            .workspace
            .as_ref()
            .map(|w| crate::ui::remote_connect::target_label(cx, &w.target));
        let workspace = self
            .owner_workspace
            .and_then(|id| crate::ui::machine_mirror::display_name_for(cx, id));
        compose_notification_title(lead.map(str::to_string), host, workspace)
    }
}

/// Ring the platform's alert sound, reporting whether one was actually made —
/// `Audible` falls back to a flash when it wasn't, so a silent `false` is the
/// difference between "you heard the bell" and "you saw it instead".
fn ring_system_bell() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSBeep();
        true
    }
    #[cfg(target_os = "windows")]
    {
        // MB_OK is the "Default Beep" scheme entry, so this follows whatever
        // the user picked in Sound Settings — including "None", which is a
        // deliberate silence and still reports success. Deliberately not
        // `Beep()`, which synthesizes a fixed tone straight at the speaker and
        // ignores the scheme. Returns immediately; the sound plays async.
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::MB_OK;
        unsafe { MessageBeep(MB_OK) != 0 }
    }
}

impl TerminalView {
    /// `name` with the suffix that says how this pane ended.
    pub(super) fn ended_title(&self, name: &str) -> String {
        let key = if self.workspace().is_some() && !self.terminal.child_exited() {
            L10nKey::PaneTitleDisconnected
        } else {
            L10nKey::PaneTitleProcessExited
        };
        t_fmt(key, &[("title", name)])
    }

    /// Takes a title the program set, and gives it to the tab only once it has
    /// stood for `TITLE_SETTLE`.
    ///
    /// Nearly every prompt framework titles the tab with the command it is
    /// about to run and puts the old title back at the next prompt — that is
    /// where a tab's process name comes from. For a command that finishes in a
    /// blink both edges land within a few frames of each other, so the label
    /// flashed the command and snapped back, which reads as a rendering glitch
    /// rather than as information. Holding the change means a command has to
    /// actually still be running to name the tab.
    ///
    /// A title that reverts before it lands never happens at all: the revert
    /// matches what the tab already shows and just drops the pending one. A
    /// program that keeps rewriting its title — a download writing progress
    /// there — still updates, because the wait already in flight adopts
    /// whatever the newest title is when it elapses rather than starting over.
    pub(super) fn set_title_when_settled(&mut self, title: String, cx: &mut Context<Self>) {
        match settle_title(&self.title, self.pending_title.is_some(), &title) {
            TitleSettle::Revert => self.pending_title = None,
            TitleSettle::Queue => self.pending_title = Some(title),
            TitleSettle::QueueAndWait => {
                self.pending_title = Some(title);
                cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(TITLE_SETTLE).await;
                    view.update(cx, |view, cx| {
                        if let Some(title) = view.pending_title.take() {
                            view.title = title;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    pub(super) fn flash_bell(&mut self, cx: &mut Context<Self>) {
        self.bell_epoch += 1;
        let epoch = self.bell_epoch;
        self.bell_flash = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |view, cx| {
                // A bell rung since this one owns the flash now. Holding
                // Backspace on an empty bash prompt rings at key-repeat rate,
                // and clearing here would blank it every few frames (#874).
                if view.bell_epoch != epoch {
                    return;
                }
                view.bell_flash = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn poll_agent_detection(&mut self, at_prompt: bool, cx: &mut Context<Self>) {
        if let Some(agent) = self
            .agent_activity
            .detect(self.terminal.foreground_agent(), at_prompt)
        {
            cx.emit(AgentDetected(agent));
        }
    }

    pub(super) fn poll_agent_status(
        &mut self,
        notify_allowed: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let replayed = self.terminal.take_replayed_agent_status();
        let session = self.terminal.agent_session();
        let update = self.agent_activity.observe(
            (self.host_id, self.pane_id),
            self.terminal.foreground_agent(),
            session.as_ref(),
            replayed,
            self.focus_handle.is_focused(window),
            notify_allowed,
            cx,
        );
        if update.session_changed {
            cx.emit(AgentSessionChanged);
        }
        if let Some(notice) = update.notice {
            if notice.delayed {
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(crate::terminal::agent_activity::DONE_SETTLE)
                        .await;
                    this.update(cx, |view, cx| {
                        view.agent_activity.notice_delivered();
                        view.notify_pane(Some(notice.agent), &notice.body, cx);
                    })
                    .ok();
                });
                self.agent_activity.await_notice(task);
            } else {
                self.notify_pane(Some(notice.agent), &notice.body, cx);
            }
        }
        if update.changed {
            cx.notify();
        }
        update.turn_finished
    }

    pub(super) fn note_integration_gap(&mut self, cx: &mut Context<Self>) {
        if self.integration_notice_shown
            || self.terminal.shell_active()
            || self.on_alt_screen()
            || self.created_at.elapsed() < INTEGRATION_GRACE
        {
            return;
        }
        self.integration_notice_shown = true;
        self.integration_notice = Some(integration_notice_message(None));
        cx.notify();

        let pane_id = self.pane_id;
        let route = self.pane_route();
        cx.spawn(async move |this, cx| {
            let fg = cx
                .background_executor()
                .spawn(async move {
                    RemoteTerminal::list_panes_on(&route)
                        .into_iter()
                        .find(|p| p.pane_id == pane_id)
                        .map(|p| p.title)
                })
                .await;
            if let Some(shim) = fg.as_deref().and_then(known_pty_shim) {
                let _ = this.update(cx, |view, cx| {
                    if view.integration_notice.is_some() {
                        view.integration_notice = Some(integration_notice_message(Some(shim)));
                        cx.notify();
                    }
                });
            }
            cx.background_executor()
                .timer(INTEGRATION_NOTICE_TIMEOUT)
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.integration_notice.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_integration_notice(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let text = self.integration_notice.clone()?;
        Some(Self::notice_pill(text, cx))
    }

    /// The one-row pill that floats over the pane's bottom-right corner.
    pub(super) fn notice_pill(text: String, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        div()
            .absolute()
            .bottom(px(GRID_PAD_Y))
            .right(px(GRID_PAD_X))
            .max_w(px(560.))
            .px_3()
            .py_1()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .child(text)
    }
}

#[cfg(test)]
mod tests;
