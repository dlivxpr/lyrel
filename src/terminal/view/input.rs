//! GPUI keyboard, committed text, and IME adaptation for the retained pane.
//! PromptInput owns editing/typeahead/composition; this adapter only routes
//! framework input and schedules the existing hold timer.

use super::*;
use gpui_kit::{Bounds, InputHandler, UTF16Selection};

pub(super) const HOLD_WINDOW: std::time::Duration = std::time::Duration::from_millis(150);

impl TerminalView {
    pub(super) fn report_focus_change(&self, focused: bool) {
        let mode = self.terminal.term.lock().mode();
        if let Some(bytes) = focus_report_bytes(mode, focused) {
            self.terminal.write(bytes);
        }
    }

    pub(super) fn on_key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let link_dropped_on_a_remote_pane =
            self.workspace().is_some() && !self.terminal.child_exited();
        if self.terminal.exited && !link_dropped_on_a_remote_pane {
            return;
        }
        if self.integration_notice.take().is_some() {
            cx.notify();
        }
        // The remote-listing failure pill goes away on the next keystroke
        // too — by then the user has seen it (#585).
        if self.completion.dismiss_notice() {
            cx.notify();
        }
        let reshaped = if cfg!(target_os = "macos") {
            crate::terminal::input::reshape_option_keystroke(
                &ev.keystroke,
                cx.global::<Config>().macos_option_as_alt,
            )
        } else {
            None
        };
        let ks = reshaped.as_ref().unwrap_or(&ev.keystroke);
        let m = &ks.modifiers;

        if self.composer_focused {
            if ks.key == "escape" {
                self.composer_escape(window, cx);
                cx.stop_propagation();
            } else if ks.key == "c" && m.control && !m.platform && !m.alt && !m.shift {
                self.composer_interrupt(window, cx);
                cx.stop_propagation();
            }
            return;
        }

        if self.search.is_some() && self.search_focused {
            if ks.key == "escape" {
                self.close_search(window, cx);
                cx.stop_propagation();
            }
            return;
        }

        if m.platform && !m.control && !m.alt {
            match self.handle_cmd_shortcut(ks, window, cx) {
                CmdKey::Consumed => {
                    cx.stop_propagation();
                    return;
                }
                CmdKey::Bubble => return,
                CmdKey::FallThrough => {}
            }
        }

        // Ctrl+V is not here: off macOS it is the `AlternatePaste` binding,
        // which the keymap withholds on the alternate screen so a full-screen
        // program gets its SYN, and which the user can retire outright. Copy
        // and cut stay, because both answer a selection this view owns and
        // fall through to the PTY when there is none.
        //
        // Ctrl+Shift+C/X are the keymap's alone: rebinding Copy has to retire
        // Ctrl+Shift+C, which it cannot if this path answers it too.
        if cfg!(not(target_os = "macos"))
            && m.control
            && !m.platform
            && !m.alt
            && !m.shift
            && matches!(ks.key.as_str(), "c" | "x")
        {
            match self.handle_cmd_shortcut(ks, window, cx) {
                CmdKey::Consumed => {
                    cx.stop_propagation();
                    return;
                }
                CmdKey::Bubble | CmdKey::FallThrough => {}
            }
        }

        if !self.accepts_input(cx) {
            return;
        }

        #[cfg(target_os = "macos")]
        if !window.has_pending_keystrokes()
            && crate::terminal::input::defer_to_ime(ks, self.key_flags())
        {
            return;
        }

        if self.input_active() {
            self.handle_editor_key(ks, cx);
            cx.stop_propagation();
            return;
        }

        // Ctrl-R landing on the PTY is only worth a notice when the user still
        // expects lyrel's menu. With the prompt editor off, the shell owning
        // Ctrl-R is exactly what was asked for.
        if m.control
            && !m.platform
            && !m.alt
            && ks.key == "r"
            && self.input.enabled()
            && cx.global::<Config>().history_search
        {
            self.note_integration_gap(cx);
        }

        use crate::terminal::prompt_input::KeyDelivery;
        let terminal = &self.terminal;
        let delivery = self.input.raw_key(ks, terminal, || {
            crate::terminal::input::KeyFlags::from_mode_with_local_conpty(
                &terminal.term.lock().mode(),
                terminal.is_local_conpty(),
            )
        });
        if !matches!(delivery, KeyDelivery::Unencoded) {
            if let KeyDelivery::Held(Some(epoch)) = delivery {
                self.arm_hold_timer(epoch, cx);
            }
            self.lifecycle.show_cursor();
            self.jump_to_prompt();
            cx.notify();
            cx.stop_propagation();
        }
    }

    pub(super) fn send_shortcut_bytes(&mut self, bytes: &[u8], key: &str, cx: &mut Context<Self>) {
        let shell_owns_prompt = self.shell_owns_prompt();
        self.input.release_hold(&self.terminal);
        self.send_to_pty(bytes, cx);
        if !shell_owns_prompt {
            self.input.shortcut(&self.terminal, key);
        }
    }

    pub(super) fn handle_cmd_shortcut(
        &mut self,
        ks: &gpui_kit::Keystroke,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CmdKey {
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "c" => {
                if self.copy_contextual(m.control, cx) {
                    CmdKey::Consumed
                } else {
                    CmdKey::FallThrough
                }
            }
            "x" => {
                if self.cut_contextual(cx) {
                    CmdKey::Consumed
                } else {
                    CmdKey::FallThrough
                }
            }
            // Cmd+V only: macOS leaves `PasteText` unbound and pastes from
            // here, and Cmd carries no control code to lose. The Ctrl+V half
            // of this key lives in the keymap as `AlternatePaste`.
            "v" => {
                self.paste_from_clipboard(cx);
                CmdKey::Consumed
            }
            "a" => {
                self.select_all_contextual(cx);
                CmdKey::Consumed
            }
            "z" => {
                self.undo_edit(m.shift, cx);
                CmdKey::Consumed
            }
            "left" => {
                if self.input_active() {
                    self.editor_move_edge(false, m.shift);
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x01], "a", cx);
                }
                CmdKey::Consumed
            }
            "right" => {
                if self.input_active() {
                    self.editor_move_edge(true, m.shift);
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x05], "e", cx);
                }
                CmdKey::Consumed
            }
            "backspace" => {
                if self.input_active() {
                    if !self.input.editor_mut().delete_selection() {
                        self.input.editor_mut().delete_to_start();
                    }
                    self.close_completion();
                    self.lifecycle.show_cursor();
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x15], "u", cx);
                }
                CmdKey::Consumed
            }
            "delete" => {
                if self.input_active() {
                    if !self.input.editor_mut().delete_selection() {
                        self.input.editor_mut().delete_to_end();
                    }
                    self.close_completion();
                    self.lifecycle.show_cursor();
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x0b], "k", cx);
                }
                CmdKey::Consumed
            }
            _ => CmdKey::Bubble,
        }
    }

    pub(super) fn handle_editor_key(&mut self, ks: &gpui_kit::Keystroke, cx: &mut Context<Self>) {
        use crate::terminal::prompt_input::EditorIntent;
        self.lifecycle.show_cursor();
        self.jump_to_prompt();
        let terminal = &self.terminal;
        match self.input.key(
            ks,
            terminal,
            &mut self.history,
            &mut self.completion,
            cx.global::<Config>().history_search,
            || {
                crate::terminal::input::KeyFlags::from_mode_with_local_conpty(
                    &terminal.term.lock().mode(),
                    terminal.is_local_conpty(),
                )
            },
        ) {
            EditorIntent::Redraw => cx.notify(),
            EditorIntent::Silent => {}
            EditorIntent::AcceptLine => self.accept_line(cx),
            EditorIntent::Submit => self.submit_command(cx),
            EditorIntent::AcceptAndSubmit => {
                self.completion_accept(cx);
                self.submit_command(cx);
            }
            EditorIntent::Handoff(bytes) => self.handoff_line_to_shell(&bytes, cx),
            EditorIntent::Commit(text) => self.commit_text(text, cx),
        }
    }

    pub(super) fn editor_move_edge(&mut self, end: bool, shift: bool) {
        self.input.editor_move_edge(end, shift);
    }

    /// The keymap context this pane declares each frame.
    ///
    /// `alt_screen` is how a binding steps aside for a full-screen program:
    /// `AlternatePaste` carries `Terminal && !alt_screen`, so Ctrl+V pastes at
    /// a prompt, reaches vim as SYN, and can still be handed the whole screen
    /// by rebinding `PasteText` onto it (#677).
    pub(in crate::terminal) fn key_context(&self, cx: &App) -> gpui_kit::KeyContext {
        let mut context = gpui_kit::KeyContext::new_with_defaults(cx.platform_info().os);
        context.add("Terminal");
        // The frame's own answer, not the terminal's. gpui matches keystrokes
        // against the context the last painted frame published, so this was
        // already a frame-old reading of the mode even when it locked; the
        // chord that must not be a frame late (`AlternatePaste`) asks the
        // terminal again in `alternate_paste`, which is what that comment
        // below is about.
        if self.frame_alt_screen {
            context.add("alt_screen");
        }
        context
    }

    pub(in crate::terminal) fn key_flags(&self) -> crate::terminal::input::KeyFlags {
        crate::terminal::input::KeyFlags::from_mode_with_local_conpty(
            &self.terminal.term.lock().mode(),
            self.terminal.is_local_conpty(),
        )
    }

    pub(super) fn tab_bytes(&self, shift: bool) -> Vec<u8> {
        crate::terminal::input::tab_bytes(shift, self.key_flags())
    }

    pub(in crate::terminal) fn send_to_pty(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.terminal.exited || !self.accepts_input(cx) {
            return;
        }
        self.terminal.write(bytes.to_vec());
        self.lifecycle.show_cursor();
        self.jump_to_prompt();
        cx.notify();
    }

    pub fn select_all_contextual(&mut self, cx: &mut Context<Self>) {
        if self.input_active() {
            self.input.editor_mut().select_all();
            cx.notify();
        } else {
            self.select_all(cx);
        }
    }

    /// Record where this frame anchors the IME, and say whether the platform
    /// has to be told. macOS asks for the rectangle itself whenever it needs
    /// one; Linux only takes it when it is pushed, and gpui pushes it on focus
    /// and around a preedit, never because the caret moved. A TUI moves the
    /// caret on its own — every redraw, every keystroke it echoes — so without
    /// a push the candidate window stayed wherever the last composition or
    /// the focus-in left it, or at the window's corner (#1131).
    pub(in crate::terminal) fn ime_anchor_moved(
        &mut self,
        anchor: Option<gpui_kit::Bounds<Pixels>>,
    ) -> bool {
        self.input.anchor_moved(anchor)
    }

    pub fn undo_edit(&mut self, redo: bool, cx: &mut Context<Self>) {
        if !self.input_active() {
            return;
        }
        if redo {
            self.input.editor_mut().redo();
        } else {
            self.input.editor_mut().undo();
        }
        self.close_completion();
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn at_shell_prompt(&self) -> bool {
        self.terminal.at_prompt()
    }

    pub fn input_active(&self) -> bool {
        self.input_inactive_reason().is_none()
    }

    /// Moves this pane between lyrel's inline editor and the shell's own line
    /// editor while it is live.
    ///
    /// A line the user already typed is not dropped on the floor: it is handed
    /// to the shell the same way an unknown chord hands one over, so the text
    /// is still on the prompt to finish under ZLE/readline. A multi-line draft
    /// has nowhere to go on a shell prompt, so it stays in the editor and comes
    /// back if the setting is turned on again.
    pub(crate) fn set_prompt_editor(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.input.enabled() == on {
            return;
        }
        if !on && self.input_active() && !self.input.editor().text().is_empty() {
            self.handoff_line_to_shell(&[], cx);
        }
        self.input.set_enabled(on);
        self.close_completion();
        self.history.cancel_search();
        cx.notify();
    }

    pub(super) fn input_inactive_reason(&self) -> Option<&'static str> {
        self.input
            .inactive_reason(&self.terminal, self.search_focused)
    }

    pub(super) fn link_inactive_reason(&self, cx: &gpui_kit::App) -> Option<&'static str> {
        (!self.accepts_input(cx)).then_some("the remote link is not attached")
    }

    pub(super) fn shell_owns_prompt(&self) -> bool {
        self.input.shell_owns_prompt(&self.terminal)
    }

    pub(crate) fn on_alt_screen(&self) -> bool {
        self.terminal.term.lock().mode().contains(Mode::ALT_SCREEN)
    }

    pub(super) fn sync_typeahead_owner(&mut self) {
        self.input.sync_owner(&self.terminal);
    }

    pub(super) fn write_gap_text(
        &mut self,
        text: &str,
        bytes: Vec<u8>,
        pasted: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(epoch) = self
            .input
            .write_gap_text(&self.terminal, text, bytes, pasted)
        {
            self.arm_hold_timer(epoch, cx);
        }
    }

    pub(super) fn arm_hold_timer(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOLD_WINDOW).await;
            let _ = this.update(cx, |view, cx| view.dump_hold(epoch, cx));
        })
        .detach();
    }

    pub(super) fn dump_hold(&mut self, epoch: u64, cx: &mut Context<Self>) {
        let accepts_input = self.accepts_input(cx);
        if self.input.timeout(&self.terminal, epoch, accepts_input) {
            cx.notify();
        }
    }

    pub(super) fn insert_newline_action(&mut self, cx: &mut Context<Self>) {
        if !self.input_active() || self.history.search().is_some() {
            cx.propagate();
            return;
        }
        self.jump_to_prompt();
        self.lifecycle.show_cursor();
        self.input.newline(&mut self.history, &mut self.completion);
        cx.notify();
    }

    pub(super) fn insert_newline_fallback_action(&mut self, cx: &mut Context<Self>) {
        if self.input_active() {
            self.insert_newline_action(cx);
        } else if (self.search.is_some() && self.search_focused)
            || self.key_flags().kitty_active()
            || !self.accepts_input(cx)
        {
            cx.propagate();
        } else {
            self.send_shortcut_bytes(self.key_flags().legacy_newline_bytes(), "enter", cx);
        }
    }

    pub(super) fn accept_line(&mut self, cx: &mut Context<Self>) {
        if self.completion.has_selection() {
            self.completion_accept(cx);
            return;
        }
        self.close_completion();
        self.submit_command(cx);
    }

    pub(super) fn submit_command(&mut self, cx: &mut Context<Self>) {
        if self.terminal.exited || !self.accepts_input(cx) {
            return;
        }
        self.input
            .submit(&self.terminal, &mut self.history, &mut self.completion);
        self.lifecycle.show_cursor();
        self.jump_to_prompt();
        cx.notify();
    }

    pub(super) fn handoff_line_to_shell(&mut self, chord: &[u8], cx: &mut Context<Self>) {
        if !self.accepts_input(cx) {
            return;
        }
        if self
            .input
            .handoff_line(&self.terminal, &mut self.completion)
        {
            self.send_to_pty(chord, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn tab_pressed(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.search_focused {
            cx.propagate();
            return;
        }
        if let Some(reason) = self.link_inactive_reason(cx) {
            log::debug!(target: "lyrel::completion", "Tab does nothing and the line stays: {reason}");
            return;
        }
        if let Some(reason) = self.input_inactive_reason() {
            log::debug!(target: "lyrel::completion", "Tab goes straight to the PTY: {reason}");
            let bytes = self.tab_bytes(!forward);
            self.send_to_pty(&bytes, cx);
            return;
        }
        self.complete_tab(forward, cx);
    }

    pub(super) fn handoff_tab_to_shell(&mut self, shift: bool, cx: &mut Context<Self>) {
        let bytes = self.tab_bytes(shift);
        self.handoff_line_to_shell(&bytes, cx);
    }

    pub fn input_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.commit_text(text, cx);
    }

    pub fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.terminal.exited || text.is_empty() || !self.accepts_input(cx) {
            return;
        }
        if self.composer_takes_typing(text, cx) {
            return;
        }
        // Typing goes to the prompt, so the view has to be looking at it.
        // Only the last branch below used to do this, which is the branch
        // taken when lyrel is *not* driving the line — so scrolling up and
        // typing did the one thing it must never do at a shell prompt:
        // accepted the characters somewhere the user could not see them.
        // `handle_editor_key` has always jumped, so Left and Backspace came
        // back to the prompt and the letters between them did not.
        self.jump_to_prompt();
        if self.history.push_search_query(text) {
            self.lifecycle.show_cursor();
            cx.notify();
            return;
        }
        if self.input_active() {
            self.input
                .commit_editor(text, &mut self.history, &mut self.completion);
            self.lifecycle.show_cursor();
            cx.notify();
            return;
        }
        self.write_gap_text(text, text.as_bytes().to_vec(), false, cx);
        self.lifecycle.show_cursor();
        cx.notify();
    }

    pub fn set_marked_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.input.mark(text);
        cx.notify();
    }

    pub fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        if self.input.unmark() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod consumer_tests {
    use super::*;
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::remote::PtySource;
    use crate::terminal::view::test_support::*;
    use gpui_kit::TestAppContext;
    mod ime_consumer_tests {
        include!("../prompt_input/ime_consumer_tests.rs");
    }
    mod routing_consumer_tests {
        include!("../prompt_input/routing_consumer_tests.rs");
    }
    mod key_consumer_tests {
        include!("../prompt_input/key_consumer_tests.rs");
    }
    /// `Ctrl-^`, which used to die in a hardcoded block that swallowed every
    /// Ctrl+digit off macOS — nothing has claimed those chords since tabs
    /// moved to Alt+1..9.
    #[gpui_kit::test]
    fn ctrl_6_reaches_the_pty_as_rs(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        cx.update(|cx| crate::ui::keymap::init(cx));
        window
            .update(cx, |view, window, cx| {
                window.activate_window();
                view.focus_handle.focus(window, cx);
            })
            .unwrap();

        let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        vcx.simulate_keystrokes("ctrl-6");

        assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x1e]));
    }

    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn cmd_backspace_reaches_a_foreground_tui_as_ctrl_u(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        window
            .update(cx, |view, window, cx| {
                assert!(
                    !view.input_active(),
                    "the foreground process, not lyrel's editor, owns input"
                );
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: gpui_kit::Keystroke {
                            modifiers: Modifiers {
                                platform: true,
                                ..Modifiers::default()
                            },
                            key: "backspace".into(),
                            key_char: None,
                        },
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .unwrap();

        assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x15]));
    }

    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn cmd_navigation_reaches_a_foreground_tui_as_readline_controls(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);

        for (key, expected) in [("left", 0x01), ("right", 0x05), ("delete", 0x0b)] {
            window
                .update(cx, |view, window, cx| {
                    assert!(!view.input_active(), "the foreground TUI owns input");
                    view.on_key_down(
                        &KeyDownEvent {
                            keystroke: gpui_kit::Keystroke {
                                modifiers: Modifiers {
                                    platform: true,
                                    ..Modifiers::default()
                                },
                                key: key.into(),
                                key_char: None,
                            },
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    );
                })
                .unwrap();
            assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![expected]));
        }
    }

    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn cmd_backspace_releases_held_input_before_ctrl_u(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Prompt {
            active: true,
            at_prompt: false,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        for _ in 0..200 {
            cx.run_until_parked();
            let gap = window
                .update(cx, |view, _, _| {
                    view.terminal.shell_active() && !view.terminal.at_prompt()
                })
                .unwrap();
            if gap {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, window, cx| {
                view.commit_text("ls", cx);
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: gpui_kit::Keystroke {
                            modifiers: Modifiers {
                                platform: true,
                                ..Modifiers::default()
                            },
                            key: "backspace".into(),
                            key_char: None,
                        },
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .unwrap();

        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"ls".to_vec()),
            "held text must reach the PTY before the line-kill"
        );
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(vec![0x15]),
            "Ctrl-U must follow the text it clears"
        );
    }
}
fn focus_report_bytes(mode: Mode, focused: bool) -> Option<&'static [u8]> {
    if !mode.contains(Mode::FOCUS_IN_OUT) {
        return None;
    }
    Some(if focused { b"\x1b[I" } else { b"\x1b[O" })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_reports_only_when_the_app_opted_in() {
        assert_eq!(focus_report_bytes(Mode::empty(), true), None);
        assert_eq!(focus_report_bytes(Mode::empty(), false), None);
        let mode = Mode::FOCUS_IN_OUT;
        assert_eq!(focus_report_bytes(mode, true), Some(b"\x1b[I".as_slice()));
        assert_eq!(focus_report_bytes(mode, false), Some(b"\x1b[O".as_slice()));
        assert_eq!(focus_report_bytes(Mode::MOUSE_MOTION, true), None);
    }
}

pub(in crate::terminal) struct TerminalInputHandler {
    view: gpui_kit::Entity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
}

impl TerminalInputHandler {
    pub(in crate::terminal) fn new(
        view: gpui_kit::Entity<TerminalView>,
        cursor_bounds: Option<Bounds<Pixels>>,
    ) -> Self {
        Self {
            view,
            cursor_bounds,
        }
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        let marked = self.view.read(cx).input.marked();
        if marked.is_empty() {
            None
        } else {
            Some(0..marked.encode_utf16().count())
        }
    }

    fn text_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _adjusted: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let text = text.to_string();
        self.view.update(cx, |view, cx| {
            view.clear_marked_text(cx);
            view.input_text(&text, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let new_text = new_text.to_string();
        self.view
            .update(cx, |view, cx| view.set_marked_text(new_text, cx));
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| view.clear_marked_text(cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let mut bounds = self.cursor_bounds?;
        let cell_width = self.view.read(cx).cell_width;
        bounds.origin.x += cell_width * range_utf16.start as f32;
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui_kit::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    fn prefers_ime_for_printable_keys(&mut self, window: &mut Window, cx: &mut App) -> bool {
        // The published Kit asks before a keystroke exists, so the option-as-
        // alt question is answered from the modifiers held right now instead
        // of the key event's set — the same predicate, read earlier.
        #[cfg(target_os = "macos")]
        if crate::terminal::input::meta_modifiers_bypass_ime(
            &window.modifiers(),
            cx.global::<Config>().macos_option_as_alt,
        ) {
            return false;
        }
        if self.view.read(cx).key_flags().reports_all_keys() {
            return false;
        }
        if window.has_pending_keystrokes() {
            return false;
        }
        true
    }
}
