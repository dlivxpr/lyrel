//! The single ordered TerminalLifecycle callback and foreground coordination.
//! Keep pre-match side effects and capability calls ordered; output notification
//! gating and batched wakeups remain owned by TerminalLifecycle.

use super::notices::ring_system_bell;
use super::*;
use gpui_kit::component::ActiveTheme as _;

impl TerminalView {
    pub(super) fn handle_event(&mut self, ev: PaneEvent, cx: &mut Context<Self>) {
        self.terminal.poll_exited();
        self.sync_typeahead_owner();
        self.poll_remote_clipboard_write(cx);
        if self.terminal.has_pending_auth() {
            cx.emit(AuthPromptReady);
        }
        match ev {
            PaneEvent::Wakeup => {
                // The grid moved under whatever the search bar last measured.
                self.note_output_under_search(cx);
                self.lifecycle.notify_output(cx);
            }
            PaneEvent::Title(title) => self.set_title_when_settled(title, cx),
            PaneEvent::ResetTitle => self.set_title_when_settled(self.default_title.clone(), cx),
            PaneEvent::PtyWrite(text) => self.terminal.write(text.into_bytes()),
            PaneEvent::Exit => {
                self.terminal.exited = true;
                // Nothing is left to settle, and a title still waiting its turn
                // would land on top of the state below a moment from now.
                self.pending_title = None;
                // The pane keeps answering to its own name (an SSH pane's
                // host, #438) — only the state suffix is localized (#602).
                self.title = self.ended_title(&self.default_title);
                if self.terminal.child_exited() {
                    cx.emit(ChildExited);
                }
                cx.notify();
            }
            PaneEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            PaneEvent::ClipboardLoad(_, fmt) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.terminal.write(fmt(&text).into_bytes());
                }
            }
            PaneEvent::ColorRequest(idx, fmt) => {
                let theme = cx.theme();
                let rgb = match idx {
                    256 => crate::terminal::palette::hsla_to_rgb(theme.foreground),
                    257 => crate::terminal::palette::hsla_to_rgb(theme.background),
                    258 => crate::terminal::palette::hsla_to_rgb(theme.caret),
                    i => self.terminal.palette[i.min(255)],
                };
                self.terminal.write(fmt(rgb).into_bytes());
            }
            PaneEvent::Bell => match cx.global::<Config>().bell {
                BellMode::None => {}
                BellMode::Visual => self.flash_bell(cx),
                BellMode::Audible => {
                    if !ring_system_bell() {
                        self.flash_bell(cx);
                    }
                }
                BellMode::Both => {
                    ring_system_bell();
                    self.flash_bell(cx);
                }
            },
            PaneEvent::TextAreaSizeRequest(fmt) => {
                let size = self.terminal.size();
                let reply = fmt(rio_vt::event::WindowSize {
                    cols: size.cols as u16,
                    rows: size.rows as u16,
                    width: (size.cols as f32 * self.cell_width.as_f32()).round() as u16,
                    height: (size.rows as f32 * self.line_height.as_f32()).round() as u16,
                });
                self.terminal.write(reply.into_bytes());
            }
        }
    }

    pub(super) fn poll_foreground(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.completion.has_work() {
            let context = self.completion_context();
            if self.completion.observe_context(context) {
                cx.notify();
            }
        }
        let notify_allowed = cx
            .global::<Config>()
            .notify_on_command_finish
            .allows(window.is_window_active());
        // Ahead of the exit check: what a program said just before its shell
        // exited is still shown.
        self.show_program_notes(notify_allowed, window, cx);
        if self.terminal.exited {
            return;
        }
        let at_prompt = self.terminal.at_prompt();

        if self.history.poll_prompt(self.history_prompt()) {
            cx.notify();
        }
        if let Some(cwd) = self.cwd() {
            self.history.rank_for_cwd(Some(&cwd));
        }

        if self.integration_notice.is_some() && self.terminal.shell_active() {
            self.integration_notice = None;
            cx.notify();
        }

        if at_prompt != self.last_at_prompt {
            self.last_at_prompt = at_prompt;
            cx.notify();
        }

        let running = !at_prompt;
        if running && self.running_agent.is_none() {
            self.running_agent = self.terminal.foreground_agent();
        }
        let cmd_finished = self.running_since.is_some() && !running;
        match (self.running_since, running) {
            (None, true) => {
                self.running_since = Some(std::time::Instant::now());
                self.running_title = self.title.clone();
                self.running_agent = self.terminal.foreground_agent();
            }
            (Some(start), false) => {
                let elapsed = start.elapsed();
                let title = std::mem::take(&mut self.running_title);
                let agent = self.running_agent.take();
                self.running_since = None;
                if notify_allowed {
                    match agent {
                        Some(_) if self.agent_activity.was_rich() => {}
                        Some(agent) => self.notify_agent_finished(agent, elapsed, cx),
                        None => {
                            let threshold = std::time::Duration::from_secs(
                                cx.global::<Config>().notify_threshold_secs,
                            );
                            if elapsed >= threshold {
                                self.notify_command_finished(&title, elapsed, cx);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        self.poll_agent_detection(at_prompt, cx);

        let turn_finished = self.poll_agent_status(notify_allowed, window, cx);

        let session = self.terminal.agent_session();
        let tool_activity = self.agent_activity.tool_activity_changed(session.as_ref());
        let cwd_now = self
            .cwd_is_on_host()
            .then(|| {
                session
                    .as_ref()
                    .and_then(|s| s.cwd.clone())
                    .or_else(|| self.cwd())
            })
            .flatten();
        let prompt_cycle = self.terminal.prompt_cycle();
        let command_finished = std::mem::replace(&mut self.git_prompt_cycle, prompt_cycle)
            != prompt_cycle
            || cmd_finished;
        if cwd_now.as_ref() != self.git_status_cwd.as_ref() || command_finished || turn_finished {
            if command_finished || turn_finished {
                self.mark_repo_changed(cwd_now.as_deref(), cx);
            }
            self.refresh_git_status(cwd_now, GitRefresh::Edge, cx);
        } else if tool_activity {
            self.refresh_git_status(cwd_now, GitRefresh::Opportunistic, cx);
        } else if self.git_retry_due(cx) {
            self.refresh_git_status(cwd_now, GitRefresh::Retry, cx);
        } else {
            self.recheck_git_root(cx);
        }

        self.follow_history_scope(cx);
    }
}

#[cfg(test)]
mod consumer_tests;
