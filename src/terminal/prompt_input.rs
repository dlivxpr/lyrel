//! Prompt input ownership, editor transitions, and composition state.
//! GPUI identity, focus, scheduling, and rendering remain on TerminalView.
pub(super) mod bytes;
pub(super) mod layout;
#[cfg(test)]
#[path = "prompt_input/owner_tests.rs"]
mod owner_tests;
use gpui_kit::{Bounds, Modifiers, Pixels};
use rio_vt::crosswords::{Mode};
use rio_vt::crosswords::grid::Dimensions;
use super::{cmd_editor::CmdEditor, completion::state::TerminalCompletion,
    history_session::TerminalHistory, hold::{GapHold, Verdict}, remote::RemoteTerminal,
    typeahead::{RawInput, Typeahead}, reverse_search};
use self::layout::{input_start, input_char_positions};

#[derive(Clone, Copy)]
pub(super) enum KeyDelivery { Unencoded, Written, Held(Option<u64>) }

pub(super) enum EditorIntent<'a> {
    Redraw,
    Silent,
    AcceptLine,
    Submit,
    AcceptAndSubmit,
    Handoff(Vec<u8>),
    Commit(&'a str),
}

pub(super) struct PromptInput {
    cmd: CmdEditor,
    enabled: bool,
    typeahead: Typeahead,
    hold: GapHold,
    last_blocked: bool,
    handoff: Option<u64>,
    interrupt_seq: Option<u64>,
    marked: String,
    anchor: Option<Bounds<Pixels>>,
    selecting: bool,
    select_gesture: bool,
    drag_word: Option<(usize, usize)>,
    goal_col: Option<usize>,
}

impl PromptInput {
    /// Writes a raw key without replaying a foreground program's input into a later prompt.
    /// The optional epoch asks the GPUI adapter to schedule the existing gap timer.
    pub(super) fn raw_key(&mut self, ks: &gpui_kit::Keystroke, terminal: &RemoteTerminal,
        flags: impl Fn() -> super::input::KeyFlags) -> KeyDelivery {
        let m = &ks.modifiers;
        if m.control && !m.platform && !m.alt && ks.key == "c" && self.handoff_active(terminal) {
            // A fresh A/B after Ctrl-C need not advance prompt_cycle: remember its report boundary.
            self.interrupt_handoff(terminal);
        }
        let Some(bytes) = super::input::keystroke_to_bytes(ks, flags()) else { return KeyDelivery::Unencoded; };
        let plain = !m.control && !m.alt && !m.platform;
        let boundary = typeahead_boundary(ks.key.as_str(), m);
        let interrupt = boundary.is_some();
        let shell_owns_prompt = self.shell_owns_prompt(terminal);
        if plain && ks.key == "backspace" && !shell_owns_prompt && self.gap_holdable(terminal)
            && let Verdict::Held(arm) = self.hold.hold_backspace(&bytes) {
            return KeyDelivery::Held(arm);
        }
        self.release_hold(terminal);
        if let Some(boundary) = boundary.filter(|_| !shell_owns_prompt) {
            // Cancel before writing Ctrl-C/D so a prompt edge cannot resurrect an owed Ctrl-U.
            self.observe(terminal, boundary);
        }
        terminal.write(bytes);
        if !shell_owns_prompt && !interrupt {
            self.observe(terminal, RawInput::Key { key: ks.key.as_str(), plain });
        }
        KeyDelivery::Written
    }
    pub(super) fn new(enabled: bool) -> Self {
        Self { cmd: CmdEditor::new(), enabled, typeahead: Typeahead::new(), hold: GapHold::new(),
            last_blocked: false, handoff: None, interrupt_seq: None, marked: String::new(),
            anchor: None, selecting: false, select_gesture: false, drag_word: None, goal_col: None }
    }
    // The existing editor capability is borrowed by history/completion and render adapters.
    pub(super) fn editor(&self) -> &CmdEditor { &self.cmd }
    pub(super) fn editor_mut(&mut self) -> &mut CmdEditor { &mut self.cmd }
    pub(super) fn enabled(&self) -> bool { self.enabled }
    pub(super) fn set_enabled(&mut self, enabled: bool) { self.enabled = enabled; }
    pub(super) fn marked(&self) -> &str { &self.marked }
    pub(super) fn mark(&mut self, text: String) { self.marked = text; }
    pub(super) fn unmark(&mut self) -> bool {
        if self.marked.is_empty() { return false; }
        self.marked.clear();
        true
    }
    pub(super) fn anchor_moved(&mut self, anchor: Option<Bounds<Pixels>>) -> bool {
        if self.anchor == anchor { return false; }
        self.anchor = anchor;
        anchor.is_some()
    }
    fn handoff_active(&self, terminal: &RemoteTerminal) -> bool {
        self.handoff == Some(terminal.prompt_cycle())
            && self.interrupt_seq.is_none_or(|seq| terminal.prompt_seq() <= seq)
            && terminal.at_prompt() && !Self::alt(terminal)
    }
    fn interrupt_handoff(&mut self, terminal: &RemoteTerminal) {
        self.interrupt_seq = Some(terminal.prompt_seq());
    }
    fn alt(terminal: &RemoteTerminal) -> bool { terminal.term.lock().mode().contains(Mode::ALT_SCREEN) }
    fn shell_vi(terminal: &RemoteTerminal) -> bool {
        terminal.shell_vi_mode() && terminal.at_prompt() && !Self::alt(terminal)
    }
    pub(super) fn shell_owns_prompt(&self, terminal: &RemoteTerminal) -> bool {
        !self.enabled || Self::shell_vi(terminal) || self.handoff_active(terminal)
    }
    pub(super) fn inactive_reason(&self, terminal: &RemoteTerminal, search_focused: bool) -> Option<&'static str> {
        if !self.enabled { return Some("the inline prompt editor is turned off"); }
        if terminal.exited { return Some("the shell has exited"); }
        if search_focused { return Some("the search field holds the keyboard"); }
        if Self::alt(terminal) { return Some("the pane is on the alternate screen"); }
        if terminal.foreground_agent().is_some() { return Some("the foreground agent owns input"); }
        if Self::shell_vi(terminal) { return Some("the shell prompt is in vi mode"); }
        if self.handoff_active(terminal) { return Some("this prompt's line was already handed to the shell"); }
        if !terminal.at_prompt() { return Some("the shell has not reported a prompt (no OSC 133)"); }
        None
    }
    fn blocked(terminal: &RemoteTerminal) -> bool {
        Self::alt(terminal) || terminal.foreground_agent().is_some() || terminal.agent_session().is_some()
    }
    pub(super) fn sync_owner(&mut self, terminal: &RemoteTerminal) {
        sync_typeahead_owner_state(&mut self.typeahead, &mut self.last_blocked, Self::blocked(terminal));
    }
    pub(super) fn observe(&mut self, terminal: &RemoteTerminal, input: RawInput<'_>) {
        observe_typeahead_for_owner(&mut self.typeahead, &mut self.last_blocked, input, Self::blocked(terminal));
    }
    pub(super) fn shortcut(&mut self, terminal: &RemoteTerminal, key: &str) {
        self.typeahead.observe(RawInput::Key { key, plain: false }, Self::alt(terminal));
    }
    fn gap_holdable(&self, terminal: &RemoteTerminal) -> bool {
        terminal.shell_active() && !Self::alt(terminal) && !self.shell_owns_prompt(terminal)
    }
    pub(super) fn settle_shell_owner(&mut self, terminal: &RemoteTerminal) {
        if let Some((_net, bytes)) = self.hold.release() { terminal.write(bytes); }
        self.typeahead.drain();
    }
    pub(super) fn flush_typeahead(&mut self, terminal: &RemoteTerminal) {
        let pasted = self.typeahead.pasted();
        let Some(seed) = self.typeahead.drain() else {
            return;
        };
        terminal.write(vec![0x15]);
        if !seed.is_empty() {
            self.prepend_into_editor(&seed, pasted);
        }
    }

    /// Take the record into the editor without paying the wipe yet. Every door
    /// into the editor opens with this.
    ///
    /// `at_prompt` comes back on the `D` mark, a whole prompt draw ahead of the
    /// `B` that arms `zle_reading`, and this editor is live for that whole
    /// window. Everything it offers rewrites the line — history recall and the
    /// ghost suggestion replace it wholesale, ⌃U empties it, completion filters
    /// on it — so the line has to be whole *before* those run, not stitched
    /// back together at submit time in front of whatever replaced it. Folding
    /// it in that late made `↑` then Enter run the recalled entry with the gap
    /// text glued to its front, and ⌃U then Enter bring back the text ⌃U had
    /// just cleared.
    ///
    /// The `^U` stays owed until `flush_typeahead`, which keeps it where it has
    /// always been on the wire: immediately before the line. Sending it here
    /// instead would put it out before the shell's own editor is reading.
    pub(super) fn adopt_typeahead(&mut self) {
        let pasted = self.typeahead.pasted();
        if let Some(seed) = self.typeahead.adopt() {
            self.prepend_into_editor(&seed, pasted);
        }
    }

    pub(super) fn wipe_pending_typeahead(&mut self, terminal: &RemoteTerminal) {
        if self.typeahead.drain().is_some() {
            terminal.write(vec![0x15]);
        }
    }
    /// Move whatever the gap hold collected into the editor's buffer, keeping
    /// the paste mark with it.
    ///
    /// The hold is the one route into that buffer that does not run through
    /// the editor: text arriving before the prompt does is kept out here and
    /// prepended when the editor takes over. A paste that lost its provenance
    /// on the way would be submitted as typed (#660) — see
    /// [`CmdEditor::prepend_pasted`].
    pub(super) fn engage_hold_into_editor(&mut self) {
        let pasted = self.hold.pasted();
        let Some(net) = self.hold.engage() else {
            return;
        };
        self.prepend_into_editor(&net, pasted);
    }

    /// Put text in front of the editor's line that the editor did not receive
    /// through its own keys, keeping the provenance that decides how the line
    /// is submitted (#660). Both routes that do this — the gap hold and the
    /// typeahead record — carry a `pasted()` to read before they hand over.
    fn prepend_into_editor(&mut self, text: &str, pasted: bool) {
        if pasted {
            self.cmd.prepend_pasted(text);
        } else {
            self.cmd.prepend_str(text);
        }
    }

    /// A gap's text on its way to the record, which replays it into the editor
    /// later — so a paste has to be recorded as one.
    fn observe_gap_text(&mut self, terminal: &RemoteTerminal, text: &str, pasted: bool) {
        self.observe(terminal, if pasted {
            RawInput::Pasted(text)
        } else {
            RawInput::Text(text)
        });
    }

    pub(super) fn release_hold(&mut self, terminal: &RemoteTerminal) {
        let pasted = self.hold.pasted();
        if let Some((net, bytes)) = self.hold.release() {
            terminal.write(bytes);
            self.observe_gap_text(terminal, &net, pasted);
        }
    }

    pub(super) fn write_gap_text(&mut self, terminal: &RemoteTerminal, text: &str, bytes: Vec<u8>, pasted: bool) -> Option<u64> {
        if self.shell_owns_prompt(terminal) { self.release_hold(terminal); terminal.write(bytes); return None; }
        if self.gap_holdable(terminal) && !text.chars().any(char::is_control) {
            let held = if pasted { self.hold.hold_pasted_text(text, &bytes) } else { self.hold.hold_text(text, &bytes) };
            if let Verdict::Held(arm) = held { return arm; }
        } else { self.release_hold(terminal); }
        terminal.write(bytes);
        self.observe_gap_text(terminal, text, pasted);
        None
    }
    pub(super) fn timeout(&mut self, terminal: &RemoteTerminal, epoch: u64, accepts_input: bool) -> bool {
        let pasted = self.hold.pasted();
        let expired = self.hold.timeout(epoch);
        if accepts_input && let Some((net, bytes)) = expired {
            terminal.write(bytes);
            self.observe_gap_text(terminal, &net, pasted);
            return true;
        }
        false
    }
    pub(super) fn handoff_line(&mut self, terminal: &RemoteTerminal, completion: &mut TerminalCompletion) -> bool {
        self.engage_hold_into_editor();
        self.adopt_typeahead();
        let line = self.cmd.text();
        if line.contains('\n') { return false; }
        completion.close();
        self.flush_typeahead(terminal);
        let tail = line.chars().count().saturating_sub(self.cmd.cursor());
        if !line.is_empty() {
            terminal.write(line.into_bytes());
            if tail > 0 {
                let left: &[u8] = if terminal.term.lock().mode().contains(Mode::APP_CURSOR) { b"\x1bOD" } else { b"\x1b[D" };
                terminal.write(left.repeat(tail));
            }
        }
        self.cmd.clear();
        self.handoff = Some(terminal.prompt_cycle());
        self.interrupt_seq = None;
        true
    }

    pub(super) fn key<'a>(&mut self, ks: &'a gpui_kit::Keystroke, terminal: &RemoteTerminal,
        history: &mut TerminalHistory, completion: &mut TerminalCompletion,
        history_search: bool, flags: impl Fn() -> super::input::KeyFlags) -> EditorIntent<'a> {
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        self.adopt_typeahead();

        let original = ks;
        let aliased;
        let ks = if m.control && !m.platform && !m.alt && matches!(key, "p" | "n") {
            aliased = gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers::default(),
                key: if key == "p" { "up" } else { "down" }.to_string(),
                key_char: None,
            };
            &aliased
        } else {
            ks
        };
        let m = &ks.modifiers;
        let key = ks.key.as_str();

        if key != "up" && key != "down" {
            self.goal_col = None;
        }
        if !(m.alt && key == ".") {
            history.reset_last_word();
        }

        if history.search().is_some() {
            match history.search_key(ks) {
                reverse_search::Action::Redraw | reverse_search::Action::Cancel => {}
                reverse_search::Action::Accept(line) => { if let Some(line) = line { self.cmd.set(&line); } }
                reverse_search::Action::Run(line) => { self.cmd.set(&line); return EditorIntent::Submit; }
            }
            return EditorIntent::Redraw;
        }

        if m.control && !m.platform && !m.alt && matches!(key, "j" | "m") {
            return EditorIntent::AcceptLine;
        }

        if completion.is_open() && !m.control && !m.alt {
            match (m.platform, key) {
                (false, "up") => {
                    completion.select(false);
                    return EditorIntent::Redraw;
                }
                (false, "down") => {
                    completion.select(true);
                    return EditorIntent::Redraw;
                }
                (false, "enter") => {
                    return EditorIntent::AcceptLine;
                }
                (true, "enter") => {
                    return EditorIntent::AcceptAndSubmit;
                }
                (false, "escape") => {
                    completion.close();

                    return EditorIntent::Redraw;
                }
                (false, "backspace") if self.cmd.selection().is_none() && !self.cmd.is_empty() => {
                    self.cmd.backspace();
                    completion.refilter(&self.cmd);

                    return EditorIntent::Redraw;
                }
                _ => {}
            }
        }

        completion.close();

        // A function key means nothing to this editor and everything to the
        // shell — PSReadLine puts CharacterSearch on F3 and HistorySearch on
        // F8, and both of those act on the line that is currently on the
        // prompt. So it takes the same route an unknown Ctrl chord takes:
        // hand the line over first, then send the key, with every modifier
        // combination going the same way (Alt+F7 is ClearHistory).
        if super::input::is_function_key(key) && !m.platform {
            if let Some(bytes) = super::input::keystroke_to_bytes(ks, flags()) {
                return EditorIntent::Handoff(bytes);
            }
        }

        if m.control && !m.platform && !m.alt {
            if cfg!(not(target_os = "macos")) {
                match key {
                    "left" => {
                        self.editor_move_h(false, m.shift, true);

                        return EditorIntent::Redraw;
                    }
                    "right" => {
                        self.editor_move_h(true, m.shift, true);

                        return EditorIntent::Redraw;
                    }
                    "backspace" => {
                        if !self.cmd.delete_selection() {
                            self.cmd.delete_word_left();
                        }
                        history.reset_navigation();

                        return EditorIntent::Redraw;
                    }
                    "delete" => {
                        if !self.cmd.delete_selection() {
                            self.cmd.delete_word_right();
                        }

                        return EditorIntent::Redraw;
                    }
                    _ => {}
                }
            }
            if cfg!(not(target_os = "macos")) && key == "a" {
                self.cmd.select_all();
                completion.close();

                return EditorIntent::Redraw;
            }
            if key == "r" && !history_search {
                return EditorIntent::Handoff(vec![0x12]);
            }
            if !self.apply_readline_ctrl(key, terminal, history, completion)
                && let Some(bytes) = super::input::keystroke_to_bytes(ks, flags()) {
                return EditorIntent::Handoff(bytes);
            }
            return EditorIntent::Redraw;
        }

        if m.alt && !m.platform && !m.control {
            match key {
                "." => {
                    history.insert_last_word(&mut self.cmd);
                    return EditorIntent::Redraw;
                }
                "b" => {
                    self.editor_move_h(false, m.shift, true);

                    return EditorIntent::Redraw;
                }
                "f" => {
                    self.editor_move_h(true, m.shift, true);

                    return EditorIntent::Redraw;
                }
                "d" => {
                    if !self.cmd.delete_selection() {
                        self.cmd.delete_word_right();
                    }
                    history.reset_navigation();

                    return EditorIntent::Redraw;
                }
                _ => {}
            }
        }

        match key {
            "enter" => {
                return EditorIntent::Submit;
            }
            "backspace" => {
                if self.cmd.is_empty() {
                    terminal.write(vec![0x7f]);
                    self.observe(terminal, RawInput::Key {
                        key: "backspace",
                        plain: true,
                    });
                    return EditorIntent::Silent;
                }
                if m.alt && self.cmd.selection().is_none() {
                    self.cmd.delete_word_left();
                } else {
                    self.cmd.backspace();
                }
                history.reset_navigation();
            }
            "delete" => {
                if m.alt {
                    self.cmd.delete_word_right();
                } else {
                    self.cmd.delete();
                }
            }
            "left" => self.editor_move_h(false, m.shift, m.alt),
            "right" => {
                if !m.shift && self.cmd.selection().is_none() {
                    if let Some(full) = completion.ghost_suggestion(&self.cmd, history) {
                        self.cmd.set(&full);

                        return EditorIntent::Redraw;
                    }
                }
                self.editor_move_h(true, m.shift, m.alt);
            }
            "home" => self.editor_move_edge(false, m.shift),
            "end" => self.editor_move_edge(true, m.shift),
            "up" => {
                if !self.editor_move_v(terminal, false, m.shift) {
                    history.previous(&mut self.cmd);
                }
                return EditorIntent::Redraw;
            }
            "down" => {
                if !self.editor_move_v(terminal, true, m.shift) {
                    history.next(&mut self.cmd);
                }
                return EditorIntent::Redraw;
            }
            "escape" => {
                let bytes = super::input::keystroke_to_bytes(ks, flags())
                    .unwrap_or_else(|| vec![0x1b]);
                terminal.write(bytes);
                return EditorIntent::Silent;
            }
            _ => {
                if !m.control && !m.platform && !m.alt {
                    if let Some(ch) = ks.key_char.as_deref() {
                        if !ch.is_empty() && ch.chars().all(|c| c >= '\u{20}' && c != '\u{7f}') {
                            return EditorIntent::Commit(original.key_char.as_deref().unwrap());
                        }
                    }
                }
                if m.alt && !m.control && !m.platform && key.chars().count() == 1 {
                    let bytes = super::input::keystroke_to_bytes(ks, flags())
                        .unwrap_or_else(|| {
                            let name = if m.shift {
                                key.to_uppercase()
                            } else {
                                key.to_string()
                            };
                            let mut b = vec![0x1b];
                            b.extend_from_slice(name.as_bytes());
                            b
                        });
                    return EditorIntent::Handoff(bytes);
                }
            }
        }
        EditorIntent::Redraw
    }

    fn apply_readline_ctrl(&mut self, key: &str, terminal: &RemoteTerminal,
        history: &mut TerminalHistory, completion: &TerminalCompletion) -> bool {
        match key {
            "r" => history.start_search(),
            "a" => {
                self.cmd.clear_selection();
                self.cmd.move_home();
            }
            "e" => {
                if self.cmd.selection().is_none()
                    && let Some(full) = completion.ghost_suggestion(&self.cmd, history)
                {
                    self.cmd.set(&full);
                } else {
                    self.cmd.clear_selection();
                    self.cmd.move_end();
                }
            }
            "b" => {
                self.cmd.clear_selection();
                self.cmd.move_left();
            }
            "f" => {
                if let Some(full) = completion.ghost_suggestion(&self.cmd, history) {
                    self.cmd.set(&full);
                } else {
                    self.cmd.clear_selection();
                    self.cmd.move_right();
                }
            }
            "w" => {
                if !self.cmd.delete_selection() {
                    self.cmd.delete_path_component_left();
                }
            }
            "u" => {
                if !self.cmd.delete_selection() {
                    self.cmd.delete_to_start();
                }
            }
            "k" => {
                if !self.cmd.delete_selection() {
                    self.cmd.delete_to_end();
                }
            }
            "h" => self.cmd.backspace(),
            "y" => self.cmd.yank(),
            "l" => {
                terminal.write(vec![0x0c]);
            }
            "c" => {
                self.cmd.clear();
                history.reset_navigation();
                let _ = self.typeahead.drain();
                let _ = self.hold.engage();
                terminal.write(vec![0x03]);
            }
            "d" => {
                if self.cmd.is_empty() {
                    self.wipe_pending_typeahead(terminal);
                    terminal.write(vec![0x04]);
                } else {
                    self.cmd.delete();
                }
            }
            _ => return false,
        }
        true
    }
    fn editor_move_h(&mut self, right: bool, shift: bool, word: bool) {
        if shift {
            self.cmd.begin_selection();
        } else if let Some((s, e)) = self.cmd.selection() {
            self.cmd.set_cursor(if right { e } else { s });
            self.cmd.clear_selection();
            return;
        }
        match (right, word) {
            (false, false) => self.cmd.move_left(),
            (false, true) => self.cmd.move_word_left(),
            (true, false) => self.cmd.move_right(),
            (true, true) => self.cmd.move_word_right(),
        }
    }

    pub(super) fn editor_move_edge(&mut self, end: bool, shift: bool) {
        if shift {
            self.cmd.begin_selection();
        } else {
            self.cmd.clear_selection();
        }
        if end {
            self.cmd.move_end();
        } else {
            self.cmd.move_home();
        }
    }

    fn editor_move_v(&mut self, terminal: &RemoteTerminal, down: bool, shift: bool) -> bool {
        let term = terminal.term.lock();
        let cursor = term.cursor().pos;
        if cursor.row.0 + (term.display_offset() as i32) < 0 { return false; }
        let scol = cursor.col.0;
        let cols = term.columns().max(1);
        drop(term);
        let chars: Vec<char> = self.cmd.text().chars().collect();
        let len = chars.len();
        let start = input_start(scol, cols);
        let (positions, end_row, end_col) = input_char_positions(&chars, start, cols);
        let end_caret = (end_row, end_col);
        let (cur_row, cur_col) = if self.cmd.cursor() < len {
            let (r, c, _) = positions[self.cmd.cursor()];
            (r, c)
        } else {
            end_caret
        };
        let mut max_row = positions
            .iter()
            .map(|&(r, _, _)| r)
            .max()
            .unwrap_or(start.0);
        if chars.last() == Some(&'\n') {
            max_row += 1;
        }
        if (down && cur_row >= max_row) || (!down && cur_row <= start.0) {
            self.goal_col = None;
            return false;
        }
        let target = if down { cur_row + 1 } else { cur_row - 1 };
        let goal = *self.goal_col.get_or_insert(cur_col);
        let mut best: Option<(usize, usize)> = None;
        for (i, &(r, c, _)) in positions.iter().enumerate() {
            if r == target {
                let dist = c.abs_diff(goal);
                if best.is_none_or(|(_, bd)| dist < bd) {
                    best = Some((i, dist));
                }
            }
        }
        if end_caret.0 == target {
            let dist = end_caret.1.abs_diff(goal);
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((len, dist));
            }
        }
        let Some((idx, _)) = best else {
            return false;
        };
        if shift {
            self.cmd.begin_selection();
        } else {
            self.cmd.clear_selection();
        }
        self.cmd.set_cursor(idx);
        true
    }

    pub(super) fn click(&mut self, idx: usize, clicks: usize, shift: bool, separators: &str, smart: bool) {
        match clicks {
            1 if shift => { self.cmd.extend_to(idx); self.selecting = true; self.drag_word = None; }
            1 => { self.cmd.set_cursor(idx); self.cmd.clear_selection(); self.selecting = true; self.drag_word = None; }
            2 => { self.cmd.select_word_at(idx, separators, smart); self.selecting = true; self.drag_word = self.cmd.selection(); }
            _ => { self.cmd.select_all(); self.selecting = false; self.drag_word = None; }
        }
        self.select_gesture = true;
        self.goal_col = None;
    }
    pub(super) fn is_selecting(&self) -> bool { self.selecting }
    pub(super) fn drag(&mut self, idx: usize, separators: &str, smart: bool) {
        if let Some((s, e)) = self.drag_word { self.cmd.extend_word_to(s, e, idx, separators, smart); }
        else { self.cmd.extend_to(idx); }
    }
    pub(super) fn end_selection(&mut self) -> bool {
        let gesture = self.select_gesture;
        self.selecting = false;
        self.select_gesture = false;
        self.drag_word = None;
        gesture
    }
    #[cfg(test)]
    pub(super) fn reset_handoff(&mut self) { self.handoff = None; }
    #[cfg(test)]
    pub(super) fn has_handoff(&self) -> bool { self.handoff.is_some() }
    #[cfg(test)]
    pub(super) fn record_for_test(&mut self, input: RawInput<'_>, blocked: bool) { self.typeahead.observe(input, blocked); }
    pub(super) fn commit_editor(&mut self, text: &str, history: &mut TerminalHistory, completion: &mut TerminalCompletion) {
        self.adopt_typeahead();
        self.cmd.insert_str(text);
        history.reset_navigation();
        self.goal_col = None;
        history.reset_last_word();
        completion.refilter(&self.cmd);
    }
    pub(super) fn paste_editor(&mut self, text: &str, history: &mut TerminalHistory, completion: &mut TerminalCompletion) {
        self.cmd.insert_pasted(text.strip_suffix('\n').unwrap_or(text));
        history.reset_navigation();
        self.goal_col = None;
        completion.close();
    }
    pub(super) fn newline(&mut self, history: &mut TerminalHistory, completion: &mut TerminalCompletion) {
        completion.close();
        self.cmd.insert_str("\n");
        history.reset_navigation();
        self.goal_col = None;
        history.reset_last_word();
    }
    pub(super) fn submit(&mut self, terminal: &RemoteTerminal, history: &mut TerminalHistory,
        completion: &mut TerminalCompletion) {
        self.engage_hold_into_editor();
        // Adoption changes the editor before editing; the owed Ctrl-U stays immediately
        // before submission, after the shell's line editor has started reading.
        self.flush_typeahead(terminal);
        let line = self.cmd.text();
        let cwd = terminal.foreground_cwd();
        let prompt = super::history_session::PromptHistory::new(
            terminal.prompt_seq(), terminal.at_prompt(), terminal.last_exit_code());
        history.record(&line, cwd, prompt);
        completion.close();
        let bracketed = terminal.term.lock().mode().contains(Mode::BRACKETED_PASTE);
        terminal.write(bytes::submit_bytes(&line, bracketed, self.cmd.pasted()));
        self.cmd.clear();
    }
}

fn sync_typeahead_owner_state(
    typeahead: &mut Typeahead,
    last_blocked: &mut bool,
    blocked: bool,
) -> bool {
    // Alternate-screen TUIs and known agents own their input. Never replay a
    // record across an ownership boundary as shell input.
    let changed = blocked != *last_blocked;
    if changed {
        typeahead.discard();
        *last_blocked = blocked;
    }
    changed
}

fn observe_typeahead_for_owner(
    typeahead: &mut Typeahead,
    last_blocked: &mut bool,
    input: RawInput<'_>,
    blocked: bool,
) {
    if !sync_typeahead_owner_state(typeahead, last_blocked, blocked) {
        typeahead.observe(input, *last_blocked);
    }
}
fn typeahead_boundary(key: &str, modifiers: &Modifiers) -> Option<RawInput<'static>> {
    if !modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    match key {
        "c" => Some(RawInput::Interrupt),
        "d" => Some(RawInput::EndOfInput),
        _ => None,
    }
}
